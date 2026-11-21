-- 154_computer_metrics.sql
--
-- Computers console (surfaces/docs/dev/computers-console-contract.mdx, D4),
-- step 2.2.
--
-- computer_metric_samples : one row per computer per minute, from the
--   `hostSample` (a HostSnapshot) a node adds to its heartbeat
--   (routes::computer_metrics). The source is what heartbeats:
--   computer_kind 'device' = runtime_devices.id, 'paired' =
--   paired_computers.id. The metrics route maps a console id onto them.
--   resolution_secs 60 = a raw sample (kept 48 hours, `snapshot` holds the
--   whole HostSnapshot for the console's lastSample); 300 = a 5 minute rollup
--   of older samples (kept 30 days, `snapshot` NULL). ts is the cloud's
--   receive time, cut to the minute (raw) or to the 5 minute bucket (rollup).
--   sample_count = raw samples behind the row (1 raw; the weight of a rollup).
--   disk_* is the fullest mount of the sample. Units: cpu/mem/disk _percent
--   0-100 (mem and disk computed at ingest from the bytes), net bytes per
--   second, *_bytes bytes; the metrics route serves the same units.
-- computer_alert_rules : offline / cpu / disk rules per owner, for one
--   computer (computer_id = a console id) or all of the owner's computers
--   (computer_id NULL). threshold: percent for cpu and disk, minutes for
--   offline. Evaluated and pushed in step 2.8.
--
-- Additive only, IF NOT EXISTS everywhere: old code keeps running on the new
-- schema. No DO blocks: the test harness splits statements.

CREATE TABLE IF NOT EXISTS public.computer_metric_samples (
    id BIGSERIAL PRIMARY KEY,
    computer_kind TEXT NOT NULL,
    computer_id TEXT NOT NULL,
    ts TIMESTAMPTZ NOT NULL,
    resolution_secs INTEGER NOT NULL DEFAULT 60,
    sample_count INTEGER NOT NULL DEFAULT 1,
    cpu_percent DOUBLE PRECISION,
    cpu_cores INTEGER,
    load1 DOUBLE PRECISION,
    mem_used_bytes BIGINT,
    mem_total_bytes BIGINT,
    mem_percent DOUBLE PRECISION,
    disk_mount TEXT,
    disk_used_bytes BIGINT,
    disk_total_bytes BIGINT,
    disk_percent DOUBLE PRECISION,
    net_rx_bytes_per_sec DOUBLE PRECISION,
    net_tx_bytes_per_sec DOUBLE PRECISION,
    uptime_sec BIGINT,
    snapshot JSONB
);
COMMENT ON COLUMN public.computer_metric_samples.cpu_percent IS 'Whole-machine CPU use, percent 0-100';
COMMENT ON COLUMN public.computer_metric_samples.mem_used_bytes IS 'Memory used, bytes (HostSnapshot.memory.usedBytes)';
COMMENT ON COLUMN public.computer_metric_samples.mem_total_bytes IS 'Memory total, bytes (HostSnapshot.memory.totalBytes)';
COMMENT ON COLUMN public.computer_metric_samples.mem_percent IS 'Memory used, percent 0-100, computed at ingest';
COMMENT ON COLUMN public.computer_metric_samples.disk_used_bytes IS 'Fullest mount used, bytes';
COMMENT ON COLUMN public.computer_metric_samples.disk_total_bytes IS 'Fullest mount size, bytes';
COMMENT ON COLUMN public.computer_metric_samples.disk_percent IS 'Fullest mount used, percent 0-100, computed at ingest';
COMMENT ON COLUMN public.computer_metric_samples.net_rx_bytes_per_sec IS 'Network received, bytes per second';
COMMENT ON COLUMN public.computer_metric_samples.net_tx_bytes_per_sec IS 'Network sent, bytes per second';
COMMENT ON COLUMN public.computer_metric_samples.uptime_sec IS 'Uptime, seconds';
CREATE UNIQUE INDEX IF NOT EXISTS uq_computer_metric_samples_slot
    ON public.computer_metric_samples (computer_kind, computer_id, resolution_secs, ts);
CREATE INDEX IF NOT EXISTS idx_computer_metric_samples_recent
    ON public.computer_metric_samples (computer_kind, computer_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_computer_metric_samples_ts
    ON public.computer_metric_samples (resolution_secs, ts);

CREATE TABLE IF NOT EXISTS public.computer_alert_rules (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    computer_id TEXT,
    rule_type TEXT NOT NULL CHECK (rule_type IN ('offline', 'cpu', 'disk')),
    threshold DOUBLE PRECISION,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_computer_alert_rules_user
    ON public.computer_alert_rules (user_id);
CREATE INDEX IF NOT EXISTS idx_computer_alert_rules_computer
    ON public.computer_alert_rules (computer_id);
