-- Files drive (contracts/files-drive/SPEC.md sections 8, 9, 15): per-computer wake
-- setting, the wake-ticket jti ledger and the storage report. Additive and
-- idempotent. Prod: applied by hand
-- (gate G-M) with a ledger row; prod skips sqlx migrations.

CREATE TABLE IF NOT EXISTS public.computer_files_wake_settings (
    computer_id TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    wake        TEXT NOT NULL DEFAULT 'ask' CHECK (wake IN ('ask', 'auto', 'never')),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One row per minted wake ticket. A ticket wakes its computer at most once
-- (woke_at); the relay never wakes, so reuse until expires_at is free.
CREATE TABLE IF NOT EXISTS public.computer_files_wake_tickets (
    jti         TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    computer_id TEXT NOT NULL,
    reason      TEXT NOT NULL,
    device_id   TEXT NOT NULL,
    issued_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at  TIMESTAMPTZ NOT NULL,
    woke_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS computer_files_wake_tickets_computer_idx
    ON public.computer_files_wake_tickets (computer_id, issued_at DESC);

-- Storage discovery (SPEC section 15): the last StorageDiscovery a runtime
-- sent in its presence heartbeat, served while the computer sleeps.
CREATE TABLE IF NOT EXISTS public.computer_files_storage (
    computer_id TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    report      JSONB NOT NULL,
    reported_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
