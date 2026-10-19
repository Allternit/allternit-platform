-- Who is signed in (email/username, never a token) and what the provider's
-- page shows about usage, read after a ready probe. usage is JSON
-- {remaining_pct, resets_at, observed_at} (+ optional windows[], stored in
-- the same JSON — no column change; see accountUsageSchema).
ALTER TABLE accounts ADD COLUMN identity TEXT;
ALTER TABLE accounts ADD COLUMN usage TEXT;
