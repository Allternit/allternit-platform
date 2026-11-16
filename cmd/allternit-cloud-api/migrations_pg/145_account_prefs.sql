-- 145_account_prefs.sql
--
-- Per-account preferences that follow the person across devices, even with
-- no computer (project folders plan, step C3). One row per (user, key); the
-- value is the whole JSON document for that key, replaced on every save.
--
--   * key 'pad_templates': the person's project pad templates and the
--     default template per mode (`routes/account_prefs.rs`,
--     GET/PUT /api/v1/account/pad-templates).
--
-- user_id is the Clerk user id (text), as in every other per-user table.
-- Erased with the account (`account_prefs` in services/account_erase.rs).
--
-- Additive only, idempotent (safe to re-run). Prod applies it through the
-- ops_applied_migrations ledger.

CREATE TABLE IF NOT EXISTS account_prefs (
    user_id     TEXT NOT NULL,
    key         TEXT NOT NULL,
    value       JSONB NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, key)
);
