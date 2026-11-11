-- Allternit Link boards that reach the account's node through the cloud
-- relay (hardware/link/README.md section 4.1). One row per board Desktop set
-- up while signed in. `token_hash` is the SHA-256 (hex) of the 32-byte relay
-- token the board presents (`x-allternit-link-board-token`); the token itself
-- is never stored or logged. The key is (account, board_id): board ids come
-- from the MAC address and can be guessed, so one account can never block
-- another's board by registering its id first. Re-registering rotates the
-- token and clears `revoked_at`; revoking makes every ticket request 401.
-- 120 was the highest number on origin/main when this was written; renumber
-- at merge if main has moved past it.
CREATE TABLE IF NOT EXISTS public.link_boards (
    account text NOT NULL,
    board_id text NOT NULL,
    runtime_device_id text NOT NULL,
    name text,
    token_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    revoked_at timestamptz,
    PRIMARY KEY (account, board_id)
);

CREATE INDEX IF NOT EXISTS idx_link_boards_board_id ON public.link_boards (board_id);
