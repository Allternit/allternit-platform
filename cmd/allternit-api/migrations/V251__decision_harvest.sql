-- Decision harvest (agency_api::decisions::harvest): opt-in training data
-- for the flywheel's trained heads (Agency Descent, Phase 1).
--
-- decision_training_optin: one row per scope ("org:<id>" or "user:<id>").
-- Nothing is harvested for a scope without opted_in = 1. Opting out deletes
-- the scope's examples.
CREATE TABLE IF NOT EXISTS decision_training_optin (
    scope TEXT PRIMARY KEY,
    opted_in INTEGER NOT NULL DEFAULT 0,
    actor TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- One example per harvested decision. Unlike `decisions` (which keeps only
-- the context hash), this keeps the DLP-redacted context text, because a
-- head cannot be trained on hashes. Labels: the outcome's label (or the
-- served choice on success) beats the teacher's confident answer.
CREATE TABLE IF NOT EXISTS decision_examples (
    decision_id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    context TEXT NOT NULL,
    question TEXT,
    options_json TEXT NOT NULL,
    teacher_backend TEXT NOT NULL,
    teacher_choice TEXT,
    teacher_confidence REAL NOT NULL,
    label TEXT,
    label_source TEXT,
    rejected_choice TEXT,
    labelled_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_decision_examples_kind ON decision_examples(kind, created_at);
CREATE INDEX IF NOT EXISTS idx_decision_examples_scope ON decision_examples(scope, created_at);
