CREATE TABLE live_ci_actions (
 id TEXT PRIMARY KEY, user_id TEXT NOT NULL, connection_id TEXT NOT NULL,
 repository TEXT NOT NULL, pull_number INTEGER NOT NULL, head_sha TEXT NOT NULL,
 action TEXT NOT NULL, run_id INTEGER, run_attempt INTEGER, expires_at INTEGER NOT NULL,
 state TEXT NOT NULL DEFAULT 'pending', created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX live_ci_actions_owner ON live_ci_actions(user_id, created_at);
CREATE UNIQUE INDEX live_ci_dispatch_once ON live_ci_actions(user_id,connection_id,repository,pull_number,head_sha,action,COALESCE(run_id,0),COALESCE(run_attempt,0)) WHERE state IN ('dispatching','submitted','unknown');
