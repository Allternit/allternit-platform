-- allternit-status D1 schema v1.

-- One row per vantage per minute. Raw probe results (JSON ProbeResult[]).
-- states: JSON {component_id: public status} for the cloudflare vantage only.
-- Kept 14 days (pruned by the daily rollup).
CREATE TABLE IF NOT EXISTS ticks (
  vantage TEXT NOT NULL,
  minute  TEXT NOT NULL,          -- 'YYYY-MM-DDTHH:MM' UTC
  ts      TEXT NOT NULL,          -- ISO time the results were taken
  results TEXT NOT NULL,
  states  TEXT,
  PRIMARY KEY (vantage, minute)
);

-- Per component per UTC day. Kept 400 days.
CREATE TABLE IF NOT EXISTS daily (
  component_id TEXT NOT NULL,
  date         TEXT NOT NULL,     -- 'YYYY-MM-DD'
  status       TEXT,              -- worst status seen that day, NULL = no data
  uptime       REAL,              -- up_checks / checks, NULL = no data
  checks       INTEGER NOT NULL DEFAULT 0,   -- ticks counted (maintenance/unknown excluded)
  up_checks    INTEGER NOT NULL DEFAULT 0,   -- ticks not down
  PRIMARY KEY (component_id, date)
);

-- State machine memory, one row per component.
CREATE TABLE IF NOT EXISTS component_state (
  component_id TEXT PRIMARY KEY,
  status       TEXT NOT NULL,     -- machine status (before maintenance overlay)
  last_change  TEXT,
  machine      TEXT NOT NULL DEFAULT '{}',  -- JSON streak counters
  alerted_at   TEXT,              -- last alert or reminder email for the open problem
  uptime_90d   REAL,              -- refreshed by the daily rollup
  updated_at   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS incidents (
  id            TEXT PRIMARY KEY,
  title         TEXT NOT NULL,
  state         TEXT NOT NULL,    -- open | resolved
  impact        TEXT NOT NULL,    -- degraded | down | maintenance
  component_ids TEXT NOT NULL,    -- JSON string[]
  started_at    TEXT NOT NULL,
  resolved_at   TEXT,
  auto          INTEGER NOT NULL DEFAULT 1,   -- 0 once a human edits
  origin        TEXT NOT NULL,    -- auto | admin
  draft         INTEGER NOT NULL DEFAULT 0,   -- 1 = admin-only until published
  kind          TEXT NOT NULL DEFAULT 'incident',  -- incident | maintenance
  ends_at       TEXT,             -- scheduled maintenance end
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS incidents_state ON incidents (state, started_at);

CREATE TABLE IF NOT EXISTS incident_updates (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  incident_id TEXT NOT NULL REFERENCES incidents (id),
  ts          TEXT NOT NULL,
  status      TEXT NOT NULL,      -- investigating | identified | monitoring | resolved
  body        TEXT NOT NULL,
  source      TEXT NOT NULL       -- auto | admin
);
CREATE INDEX IF NOT EXISTS incident_updates_incident ON incident_updates (incident_id, id);

-- Admin actions and failed admin logins (rate limit reads these).
CREATE TABLE IF NOT EXISTS audit (
  id     INTEGER PRIMARY KEY AUTOINCREMENT,
  ts     TEXT NOT NULL,
  actor  TEXT NOT NULL,
  action TEXT NOT NULL,
  target TEXT,
  detail TEXT,
  ip     TEXT
);
CREATE INDEX IF NOT EXISTS audit_ip ON audit (action, ip, ts);
