-- O15 (WP-C1): one cost ledger for every model call.
--
-- llm_usage_events becomes the single ledger: the gateway, internal
-- gizzi_completion calls (memory extraction, curation, agency, templates,
-- lessons), gizzi-code's own calls (reported over POST /api/v1/usage/ledger)
-- and S1 decisions (cost 0, still counted) all write one row per model call.
-- Existing columns cover tokens (prompt/completion/reasoning), cache reads
-- (cached_tokens), cost and latency_ms; this adds the attribution keys.
--
-- source:      gateway | internal | gizzi | s1 (which writer produced the row;
--              rows written before V207 are NULL = gateway).
-- surface:     chat | cowork | code | browser | design | bot | agency | template
--              | memory | lessons | s1 | batch | api | internal.
-- tier:        S0 | S1 | S2 | S3.
-- lane:        api | subscription-cli | local.
-- s1_incumbent_cost_microdollars: for S1 decisions, the estimated cost the
--              incumbent (S2) decider would have had; summed as estimated
--              savings only where decision_served_by_s1 = 1.

ALTER TABLE llm_usage_events ADD COLUMN source TEXT;
ALTER TABLE llm_usage_events ADD COLUMN surface TEXT;
ALTER TABLE llm_usage_events ADD COLUMN run_id TEXT;
ALTER TABLE llm_usage_events ADD COLUMN node_id TEXT;
ALTER TABLE llm_usage_events ADD COLUMN tier TEXT;
ALTER TABLE llm_usage_events ADD COLUMN lane TEXT;
ALTER TABLE llm_usage_events ADD COLUMN cache_write_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE llm_usage_events ADD COLUMN decision_served_by_s1 INTEGER NOT NULL DEFAULT 0;
ALTER TABLE llm_usage_events ADD COLUMN s1_incumbent_cost_microdollars INTEGER;

CREATE INDEX IF NOT EXISTS idx_llm_usage_events_surface
    ON llm_usage_events(tenant_id, surface, created_at);
CREATE INDEX IF NOT EXISTS idx_llm_usage_events_run
    ON llm_usage_events(run_id);
CREATE INDEX IF NOT EXISTS idx_llm_usage_events_session_source
    ON llm_usage_events(gizzi_session_id, source);
