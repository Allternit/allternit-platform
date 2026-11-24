-- Files drive key sharing (contracts/files-drive/SPEC.md section 13).
-- cloud-api keeps only public keys and sealed envelopes. It never sees the
-- Files root key (FRK) and cannot open an envelope. Tables only, idempotent.
-- Prod: applied by hand (gate G-M) with a ledger row; prod skips sqlx
-- migrations. Until it is applied every /api/v1/files-keys route answers
-- feature_disabled.

-- One row per FRK generation. The current key is the one with no
-- `retired_at`; a rotation retires it and inserts the next. Old rows and
-- their envelopes stay so replicas sealed with an old key still open until
-- they are re-sealed.
CREATE TABLE IF NOT EXISTS files_keys (
    user_id            TEXT NOT NULL,
    key_id             TEXT NOT NULL,
    rotated_from       TEXT,
    created_by_device  TEXT NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    retired_at         TIMESTAMPTZ,
    -- Set when a device holding this key is removed: the next device with
    -- the key should rotate.
    rotation_needed    BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (user_id, key_id)
);

-- At most one current key per user, so two first devices racing to create
-- the key cannot both win.
CREATE UNIQUE INDEX IF NOT EXISTS files_keys_one_current_idx
    ON files_keys (user_id) WHERE retired_at IS NULL;

-- Devices (phones, browsers, Desktop, computers under their runtime device
-- id) that were granted the key. `device_pub` is the X25519 public key a
-- rotation seals the next key to. Removing a device deletes its row.
CREATE TABLE IF NOT EXISTS files_key_devices (
    user_id      TEXT NOT NULL,
    device_id    TEXT NOT NULL,
    device_name  TEXT NOT NULL DEFAULT '',
    device_pub   TEXT NOT NULL,
    approved_by  TEXT,
    added_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, device_id)
);

-- `KeyEnvelope`: the FRK wrapped to one device. Opaque to cloud-api.
CREATE TABLE IF NOT EXISTS files_key_envelopes (
    user_id     TEXT NOT NULL,
    key_id      TEXT NOT NULL,
    device_id   TEXT NOT NULL,
    envelope    JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, key_id, device_id)
);

-- `KeyShareRequest`s. Pending for 10 minutes; a grant consumes one. Rows
-- are kept (state granted/expired/replaced) for the request rate limit.
CREATE TABLE IF NOT EXISTS files_key_requests (
    request_id    TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    device_id     TEXT NOT NULL,
    device_name   TEXT NOT NULL DEFAULT '',
    device_pub    TEXT NOT NULL,
    confirm_code  TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at    TIMESTAMPTZ NOT NULL,
    state         TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'granted', 'replaced')),
    granted_by    TEXT,
    granted_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS files_key_requests_user_idx
    ON files_key_requests (user_id, created_at DESC);

-- One pending request per device: a new request from the same device
-- replaces the old one.
CREATE UNIQUE INDEX IF NOT EXISTS files_key_requests_one_pending_idx
    ON files_key_requests (user_id, device_id) WHERE state = 'pending';
