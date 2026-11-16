-- Files drive sharing (contracts/files-drive/SPEC.md sections 2, 8, 9.6, 14):
-- shares, their guests, the per-access audit, and upload-only drops queued
-- while the computer sleeps. Idempotent. Prod: applied by hand (gate G-M) with
-- a ledger row; prod skips sqlx migrations.

-- One row per share link. `link_token` is a random 160-bit URL-safe id (the
-- `/s/<token>` link the owner can copy again, like thread_shares.token).
-- `passcode_hash` is PBKDF2-HMAC-SHA256 (`pbkdf2-sha256$<iter>$<salt>$<hash>`),
-- never the passcode. Revoking sets `revoked_at`; the gateway checks it on
-- every guest request.
CREATE TABLE IF NOT EXISTS files_shares (
    share_id           TEXT PRIMARY KEY,
    owner_user_id      TEXT NOT NULL,
    owner_display_name TEXT NOT NULL DEFAULT '',
    computer_id        TEXT NOT NULL,
    root_id            TEXT NOT NULL,
    item_id            TEXT NOT NULL,
    item_name          TEXT NOT NULL DEFAULT '',
    role               TEXT NOT NULL CHECK (role IN ('view_download', 'upload_only', 'edit', 'comment')),
    link_token         TEXT NOT NULL UNIQUE,
    passcode_hash      TEXT,
    guest_wake         BOOLEAN NOT NULL DEFAULT FALSE,
    created_by         TEXT NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at         TIMESTAMPTZ,
    revoked_at         TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS files_shares_computer_idx
    ON files_shares (computer_id, created_at DESC);

-- One row per redeemed guest session chain. Revoking one guest leaves the
-- share working for everyone else.
CREATE TABLE IF NOT EXISTS files_share_guests (
    guest_id     TEXT PRIMARY KEY,
    share_id     TEXT NOT NULL REFERENCES files_shares (share_id) ON DELETE CASCADE,
    guest_name   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS files_share_guests_share_idx
    ON files_share_guests (share_id);

-- `ShareAccess`: one row per guest access, allowed or denied. Redeem attempts
-- made before a guest exists carry guest_id 'unredeemed'.
CREATE TABLE IF NOT EXISTS files_share_access (
    id       BIGSERIAL PRIMARY KEY,
    share_id TEXT NOT NULL,
    guest_id TEXT NOT NULL,
    action   TEXT NOT NULL CHECK (action IN ('redeem', 'list', 'download', 'upload', 'comment', 'wake')),
    item_id  TEXT,
    result   TEXT NOT NULL CHECK (result IN ('allowed', 'denied')),
    error    TEXT,
    at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS files_share_access_share_idx
    ON files_share_access (share_id, at DESC);

-- `QueuedDrop`: an upload-only drop to a sleeping computer, stored in the
-- owner's R2 space at files-drops/<computer_id>/<drop_id>.
CREATE TABLE IF NOT EXISTS files_queued_drops (
    drop_id       TEXT PRIMARY KEY,
    share_id      TEXT NOT NULL REFERENCES files_shares (share_id) ON DELETE CASCADE,
    guest_id      TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    computer_id   TEXT NOT NULL,
    parent_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    size          BIGINT NOT NULL,
    content_hash  TEXT NOT NULL,
    r2_key        TEXT NOT NULL,
    received_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    state         TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'delivered', 'rejected'))
);

CREATE INDEX IF NOT EXISTS files_queued_drops_computer_idx
    ON files_queued_drops (computer_id, state, received_at);
