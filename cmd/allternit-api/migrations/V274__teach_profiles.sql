-- Teach: one business profile per user (reusable values like erp_url, reports_root).
CREATE TABLE IF NOT EXISTS teach_profiles (
    user_id TEXT PRIMARY KEY,
    body_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
