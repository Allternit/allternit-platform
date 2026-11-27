-- Knowledge factory (contract: handoffs/memory-raw-rag-knowledge-factory-2026-11-27/CONTRACT.md).
-- Agents file flags (missing / wrong / stale knowledge); a curator or agent
-- turns them into proposals; the person approves every proposal.
-- Additive only: two new tables and their indexes.
CREATE TABLE IF NOT EXISTS knowledge_flags (
  id TEXT PRIMARY KEY, user_id TEXT NOT NULL, agent_id TEXT, session_id TEXT, turn_id TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('missing','wrong','stale')),
  target_kind TEXT NOT NULL CHECK (target_kind IN ('skill','domain','data_access')),
  target_ref TEXT, evidence TEXT NOT NULL, suggested_fix TEXT,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','proposed','resolved','dismissed')),
  proposal_id TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now')));
CREATE INDEX IF NOT EXISTS idx_knowledge_flags_user_status ON knowledge_flags(user_id, status);
CREATE TABLE IF NOT EXISTS knowledge_proposals (
  id TEXT PRIMARY KEY, user_id TEXT NOT NULL,
  target_kind TEXT NOT NULL CHECK (target_kind IN ('skill','domain','data_access')),
  target_ref TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL, base TEXT,
  evidence_json TEXT NOT NULL DEFAULT '[]',
  origin TEXT NOT NULL CHECK (origin IN ('curator','agent','skill_growth')),
  eval_json TEXT,
  status TEXT NOT NULL DEFAULT 'proposed' CHECK (status IN ('proposed','approved','rejected','active','rolled_back')),
  decided_at TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now')));
CREATE INDEX IF NOT EXISTS idx_knowledge_proposals_user_status ON knowledge_proposals(user_id, status);
