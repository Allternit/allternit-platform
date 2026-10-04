-- Photo avatars included with a plan (routes::photo_avatars). One row per attempt on the
-- Allternit-paid lane: reserved before the provider call, then marked used (an image came
-- back) or released (it didn't). The monthly allowance counts `used` rows plus `reserved`
-- rows younger than 10 minutes in the same UTC calendar month. The source photo is never
-- stored. (43 is mcp_oauth_approvals.)
CREATE TABLE IF NOT EXISTS photo_avatar_usage (
    id                  TEXT PRIMARY KEY,
    user_id             TEXT NOT NULL,
    period_start        DATE NOT NULL,                 -- first day of the UTC month
    status              TEXT NOT NULL DEFAULT 'reserved'
                        CHECK (status IN ('reserved', 'used', 'released')),
    plan_id             TEXT NOT NULL,
    style               TEXT NOT NULL,
    provider            TEXT NOT NULL,
    model               TEXT NOT NULL,
    cost_estimate_usd   DOUBLE PRECISION NOT NULL,
    output_bytes        INTEGER,
    failure             TEXT,
    consent_attested_at TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    settled_at          TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS photo_avatar_usage_user_period ON photo_avatar_usage (user_id, period_start);
