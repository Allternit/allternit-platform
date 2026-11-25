-- Share-link usage meter (Apps + MCP 2026, plan v3 lane 29, D8). Additive only.
-- Bytes served per live link per UTC month, fed by OpenTunnel analytics events
-- (POST /api/v1/internal/share-links/usage). The 20 GB/month cap reads this
-- table. share_links.bytes_month mirrors the current month for display.
CREATE TABLE IF NOT EXISTS share_link_usage (
  link_id    TEXT NOT NULL REFERENCES share_links(id) ON DELETE CASCADE,
  period     TEXT NOT NULL,                 -- 'YYYY-MM' (UTC)
  bytes      BIGINT NOT NULL DEFAULT 0,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (link_id, period)
);
-- One row per analytics event already counted: a replay inserts nothing.
CREATE TABLE IF NOT EXISTS share_link_usage_events (
  event_id    TEXT PRIMARY KEY,
  link_id     TEXT NOT NULL REFERENCES share_links(id) ON DELETE CASCADE,
  bytes       BIGINT NOT NULL,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
