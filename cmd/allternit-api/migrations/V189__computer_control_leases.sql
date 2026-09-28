-- ACI P4 (docs/ACI_P4_COMPUTERS_SPEC.md in allternit-ai): one controller per
-- computer at a time, across devices. Whoever holds the lease may send input;
-- everyone else watches. Leases expire unless renewed, so a closed phone
-- can't lock a computer. See src/computer_control_lease.rs.
CREATE TABLE IF NOT EXISTS computer_control_leases (
    computer_id TEXT PRIMARY KEY,
    -- user | agent | session | bot
    holder_kind TEXT NOT NULL,
    holder_id TEXT NOT NULL,
    holder_label TEXT,
    -- The device a user holds control from (Settings → Cowork device id).
    device_id TEXT,
    acquired_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
