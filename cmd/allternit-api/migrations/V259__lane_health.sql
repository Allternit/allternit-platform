-- V259: lane health samples (`src/netops/lanes.rs`, `subscription_sync.rs`).
--
-- One row each time a lane's state changes (green / amber / red), written by the
-- Subscriptions sync from the gateway's `GET /v1/lanes`. The status board reads the
-- newest row per lane and walks back to when the current state began ("since").
-- Rows are kept 14 days.
CREATE TABLE IF NOT EXISTS lane_samples (
    lane_id TEXT NOT NULL,
    owner   TEXT NOT NULL,
    state   TEXT NOT NULL,
    reason  TEXT,
    label   TEXT,
    at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lane_samples_lane ON lane_samples(owner, lane_id, at);
