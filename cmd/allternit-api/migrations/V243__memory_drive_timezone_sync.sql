-- Nightly Dream runs at night in the owner's time zone (IANA name, e.g.
-- America/Chicago). NULL = server time.
ALTER TABLE memory_drives ADD COLUMN timezone TEXT;
-- Two-way sync between the owner's computers (no main copy). The Desktop
-- app runs the merge; this remembers, per peer computer, the revisions both
-- sides had after the last successful sync (the merge base), so deletes on
-- one side are not brought back by the other.
CREATE TABLE memory_drive_peers (
    drive_id TEXT NOT NULL REFERENCES memory_drives(id),
    peer_id TEXT NOT NULL,
    peer_name TEXT,
    local_revision TEXT,
    peer_revision TEXT,
    last_sync_at TEXT,
    last_error TEXT,
    PRIMARY KEY (drive_id, peer_id)
);
