-- Kernel UI backend (agent rules, routing policy, decision types, templates).
-- WRITTEN, NOT APPLIED to any production DB. All statements are idempotent:
-- the module also runs them on first use so a fresh node works without a
-- separate migration step.
--
-- * kernel_ui_docs            : one JSON document per (kind, scope); kind is
--   'rules' or 'routing'. doc_json is a flat {field path: value} override map.
-- * kernel_ui_decision_types  : custom decision types (always shadow, advisory).
-- * kernel_ui_decision_status : explicit per-type status (shadow|live|disabled).
--   Cleared whenever the S1 backend changes, which returns every type to shadow.
-- * kernel_ui_templates       : run templates, one JSON document per template.
CREATE TABLE IF NOT EXISTS kernel_ui_docs (
    kind        TEXT NOT NULL,
    scope       TEXT NOT NULL,
    doc_json    TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (kind, scope)
);
CREATE TABLE IF NOT EXISTS kernel_ui_decision_types (
    id          TEXT NOT NULL,
    owner_id    TEXT NOT NULL,
    doc_json    TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (owner_id, id)
);
CREATE TABLE IF NOT EXISTS kernel_ui_decision_status (
    id          TEXT PRIMARY KEY,
    status      TEXT NOT NULL CHECK (status IN ('shadow', 'live', 'disabled')),
    updated_at  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS kernel_ui_templates (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL,
    scope       TEXT NOT NULL,
    doc_json    TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS kernel_ui_templates_owner ON kernel_ui_templates (owner_id, scope);
