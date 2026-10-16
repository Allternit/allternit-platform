-- V258: attention items, the durable "something needs fixing" record (`src/netops/alerts.rs`).
--
-- Every unhappy outcome the server sees (a failed or missed routine, a routine
-- with no bot, a lost run record, a red lane, a held hop …) opens one row here.
-- The id is deterministic (`<kind>:<source_id>`), so opening the same problem
-- again bumps `last_seen_at` instead of adding a row. Each open and resolve also
-- writes `attention.opened` / `attention.resolved` to `runtime_user_events`, which
-- the forwarder sends to the cloud (push). `renotify_at` drives the re-notify
-- sweep until `resolved_at` is set.
CREATE TABLE IF NOT EXISTS attention_items (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    kind TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'warn',
    bot_id TEXT,
    thread_id TEXT,
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    detail_json TEXT NOT NULL DEFAULT '{}',
    provenance_json TEXT,
    retry_json TEXT,
    attempts INTEGER NOT NULL DEFAULT 1,
    opened_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    renotify_at TEXT,
    resolved_at TEXT,
    resolution TEXT,
    -- 1 when a critical item still owes the owner a text (sent by the sweep only
    -- when `alerts.criticalSms` is on; cleared either way).
    sms_due INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_attention_items_open ON attention_items(owner, resolved_at);
CREATE INDEX IF NOT EXISTS idx_attention_items_renotify ON attention_items(renotify_at) WHERE resolved_at IS NULL;

-- Why a run ended the way it did (`timeout`, `no_bot`, `lane_blocked`, `model_error`,
-- `quota`, `runtime_restart`, `missed_runtime_offline`), when it was alerted on, and
-- how late a scheduled run started.
ALTER TABLE routine_runs ADD COLUMN outcome_reason TEXT;
ALTER TABLE routine_runs ADD COLUMN alerted_at TEXT;
ALTER TABLE routine_runs ADD COLUMN late_by_ms INTEGER;
