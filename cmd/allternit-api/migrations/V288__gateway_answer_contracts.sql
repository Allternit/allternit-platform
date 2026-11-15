CREATE TABLE gateway_answer_turns (
 remote_binding_id TEXT NOT NULL, correlation_id TEXT NOT NULL,
 policy_json TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending',
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 PRIMARY KEY(remote_binding_id,correlation_id)
);
