-- V262: injection defense core (`src/netops/provenance.rs`, `src/autonomy.rs`).
--
-- Every input that reaches a thread gets a trust label. `content_provenance`
-- records each one (the excerpt is stored only as a hash). `bot_threads.trust_floor`
-- is the lowest trust seen on the thread and only ever goes down. Every outbound
-- send is checked for secrets, PII and unknown destinations; each verdict lands in
-- `egress_decisions`. `provenance_json` rides on email drafts so the card can say
-- where the proposal came from.
ALTER TABLE bot_threads ADD COLUMN trust_floor TEXT;

CREATE TABLE IF NOT EXISTS content_provenance (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL DEFAULT '',
    thread_id TEXT NOT NULL,
    bot_id TEXT,
    source_kind TEXT NOT NULL,
    trust TEXT NOT NULL,
    source_ref TEXT NOT NULL DEFAULT '',
    score INTEGER NOT NULL DEFAULT 0,
    flags_json TEXT NOT NULL DEFAULT '[]',
    excerpt_hash TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_content_provenance_thread ON content_provenance(thread_id, created_at);

CREATE TABLE IF NOT EXISTS egress_decisions (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    thread_id TEXT,
    channel TEXT NOT NULL,
    destination TEXT NOT NULL DEFAULT '',
    -- allow | ask | block
    verdict TEXT NOT NULL,
    reasons_json TEXT NOT NULL DEFAULT '[]',
    dlp_ids_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_egress_decisions_owner ON egress_decisions(owner, created_at);

ALTER TABLE agent_email_outbound ADD COLUMN provenance_json TEXT;
