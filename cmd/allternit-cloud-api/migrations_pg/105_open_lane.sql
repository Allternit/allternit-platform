-- 105_open_lane.sql
--
-- Open lane (open-weight `-open` models) and refusal handoff
-- (`services/open_lane.rs`, `routes/open_lane.rs`).
--
--   * user_open_lane: one row per person who opened Settings > Open models.
--     No row = acceptable use not accepted, age not confirmed, handoff off.
--     Open-lane models answer 403 open_lane_terms_required until both
--     aup_accepted_at and age_confirmed_at are set; handoff_enabled can only
--     be true while both are set.
--   * open_lane_blocks: one row per hard-line block (csam,
--     mass_casualty_weapons, real_person_sexual). Category only — the prompt
--     text is never stored.
--
-- Additive only, idempotent (safe to re-run).

CREATE TABLE IF NOT EXISTS public.user_open_lane (
    user_id text PRIMARY KEY,
    aup_accepted_at timestamp with time zone,
    age_confirmed_at timestamp with time zone,
    handoff_enabled boolean NOT NULL DEFAULT false,
    updated_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS public.open_lane_blocks (
    id bigserial PRIMARY KEY,
    user_id text NOT NULL,
    category text NOT NULL,
    created_at timestamp with time zone NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_open_lane_blocks_user
    ON public.open_lane_blocks (user_id, created_at DESC);
