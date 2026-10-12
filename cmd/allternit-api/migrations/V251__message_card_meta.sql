-- Interactive answers: per assistant message, what the server found when it
-- checked the message's cards (diagnostics + text fallback) and what the user
-- last set on each card (sliders, fields, checklists), so a card comes back
-- the same on every device. Keyed by user, chat session and gizzi message id.
CREATE TABLE IF NOT EXISTS message_card_meta (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    checks TEXT NOT NULL DEFAULT '[]',
    state TEXT NOT NULL DEFAULT '{}',
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, session_id, message_id)
);
CREATE INDEX IF NOT EXISTS idx_message_card_meta_session ON message_card_meta (user_id, session_id);
