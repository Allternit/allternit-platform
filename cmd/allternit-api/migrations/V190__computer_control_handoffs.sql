-- ACI P4 hand-offs (Eoj 2026-09-28: "should offer hand offs"). Control of a
-- computer changes hands by offer and accept, except that a person can always
-- take over from an agent. A `request` asks the current holder for control;
-- an `offer` is the holder handing control to someone. Accepting moves the
-- lease (computer_control_leases, V189). See src/computer_control_lease.rs.
CREATE TABLE IF NOT EXISTS computer_control_handoffs (
    id TEXT PRIMARY KEY,
    computer_id TEXT NOT NULL,
    -- request | offer
    direction TEXT NOT NULL,
    from_kind TEXT NOT NULL,
    from_id TEXT NOT NULL,
    from_label TEXT,
    from_device_id TEXT,
    to_kind TEXT NOT NULL,
    to_id TEXT NOT NULL,
    to_label TEXT,
    to_device_id TEXT,
    note TEXT,
    -- pending | accepted | declined | cancelled | expired
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_computer_control_handoffs_pending ON computer_control_handoffs(computer_id, status);
