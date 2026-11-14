-- Platform API: the stuck-response sweep (`responses::fail_abandoned`, every
-- 5 minutes) looks for rows still `in_progress` past the turn cap. Only running
-- rows are indexed, so the index stays tiny and the sweep is an index scan.
CREATE INDEX IF NOT EXISTS idx_platform_responses_in_progress
    ON public.platform_responses (created_at)
    WHERE status = 'in_progress';
