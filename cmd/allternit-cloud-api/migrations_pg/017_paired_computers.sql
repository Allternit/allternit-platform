-- 017_paired_computers.sql
--
-- ACI P4 remote computers (allternit-ai docs/ACI_P4_COMPUTERS_SPEC.md):
-- a user pairs one of their own machines so it can be viewed and taken over
-- from any of their devices over the Allternit mesh. The record lives here,
-- at account level, so every device (phone included) sees the same list;
-- each runtime mirrors it as a `fabric` computer for control and VNC.
--
-- computer_pairing_codes : short-lived, single-use codes shown in the
--                          Computers list. Stored hashed.
-- paired_computers       : a paired machine, its mesh address once it has
--                          joined, and whether its VNC server is up. The
--                          machine authenticates its reports with a secret
--                          issued at pairing (stored hashed).

CREATE TABLE IF NOT EXISTS public.computer_pairing_codes (
    code_hash text PRIMARY KEY,
    user_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    used_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_computer_pairing_codes_user ON public.computer_pairing_codes (user_id);

CREATE TABLE IF NOT EXISTS public.paired_computers (
    id text PRIMARY KEY,
    user_id text NOT NULL,
    name text NOT NULL,
    os text,
    secret_hash text NOT NULL,
    mesh_ip text,
    vnc_ready boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_paired_computers_user ON public.paired_computers (user_id);
