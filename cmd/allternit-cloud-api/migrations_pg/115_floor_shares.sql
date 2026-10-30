-- Office view live link: one public floor per account. `projection` holds only
-- the allowlisted PublicFloor (routes/floor_shares.rs); a disabled share keeps
-- no projection. Re-enabling rotates the token so old links stay dead.
CREATE TABLE IF NOT EXISTS floor_shares (
    token TEXT PRIMARY KEY,
    user_id TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    projection JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
