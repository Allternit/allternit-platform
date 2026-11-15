-- Step 13: W3C traceparent on the rows a trace passes through, so one trace
-- runs session -> turn -> tool_call -> worker result across requests.
-- Nullable and additive; safe on a database that already has rows.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS traceparent TEXT;
ALTER TABLE work_items ADD COLUMN IF NOT EXISTS traceparent TEXT;
ALTER TABLE tool_calls ADD COLUMN IF NOT EXISTS traceparent TEXT;
