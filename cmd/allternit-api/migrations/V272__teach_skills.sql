-- Teach v2: skills taught from a recording (intent, steps, inputs), per user.
CREATE TABLE IF NOT EXISTS teach_skills (
    id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    contract_id TEXT,
    body_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX IF NOT EXISTS idx_teach_skills_user_updated ON teach_skills(user_id, updated_at DESC);
