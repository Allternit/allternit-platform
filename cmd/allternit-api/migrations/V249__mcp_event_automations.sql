-- Transport subscriptions and automation rules have independent identities.
CREATE TABLE mcp_event_rules (
 id TEXT PRIMARY KEY, subscription_id TEXT NOT NULL, user_id TEXT NOT NULL,
 version INTEGER NOT NULL DEFAULT 1, config TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX mcp_event_rules_subscription ON mcp_event_rules(subscription_id,user_id);
CREATE TABLE mcp_event_jobs (
 id TEXT PRIMARY KEY, rule_id TEXT NOT NULL, subscription_id TEXT NOT NULL, user_id TEXT NOT NULL, connector_id TEXT NOT NULL,
 rule_version INTEGER NOT NULL, config TEXT NOT NULL, status TEXT NOT NULL,
 ready_at INTEGER NOT NULL, event_ids TEXT NOT NULL, payload TEXT NOT NULL,
 ticket_id TEXT, thread_id TEXT, session_id TEXT, output TEXT, error TEXT,
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX mcp_event_jobs_ready ON mcp_event_jobs(status,ready_at);
CREATE INDEX mcp_event_jobs_owner ON mcp_event_jobs(user_id,subscription_id,created_at);
CREATE TABLE mcp_event_receipts (
 subscription_id TEXT NOT NULL,event_id TEXT NOT NULL, payload TEXT NOT NULL,
 received_at INTEGER NOT NULL, PRIMARY KEY(subscription_id,event_id)
);
ALTER TABLE mcp_event_subscriptions ADD COLUMN payload_schema TEXT;
ALTER TABLE mcp_event_subscriptions ADD COLUMN replay_truncated INTEGER NOT NULL DEFAULT 0;
CREATE TABLE mcp_event_attempts (
 id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL, rule_id TEXT NOT NULL,
 started_at INTEGER NOT NULL, actor TEXT NOT NULL DEFAULT 'runtime',
 status TEXT NOT NULL DEFAULT 'running',thread_id TEXT,session_id TEXT,output TEXT,error TEXT
);
CREATE INDEX mcp_event_attempts_rule ON mcp_event_attempts(rule_id,started_at);
CREATE TABLE mcp_event_job_actions (
 id INTEGER PRIMARY KEY AUTOINCREMENT,job_id TEXT NOT NULL,user_id TEXT NOT NULL,
 action TEXT NOT NULL,at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
ALTER TABLE mcp_event_receipts ADD COLUMN ticket_id TEXT;

CREATE INDEX mcp_event_jobs_connector ON mcp_event_jobs(user_id,connector_id,subscription_id);
ALTER TABLE mcp_event_attempts ADD COLUMN agent_run_id TEXT;
ALTER TABLE mcp_event_attempts ADD COLUMN started_ms INTEGER;
CREATE TABLE mcp_event_notices (
 id TEXT PRIMARY KEY, bot_id TEXT NOT NULL, thread_id TEXT, session_id TEXT,
 run_id TEXT NOT NULL,event_type TEXT NOT NULL,payload TEXT NOT NULL,delivered INTEGER NOT NULL DEFAULT 0
);
