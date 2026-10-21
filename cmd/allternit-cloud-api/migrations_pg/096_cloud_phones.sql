-- 096_cloud_phones.sql
--
-- Cloud Android phones (redroid) that show up as Phone Link devices.
-- Off by default: nothing here is reachable until CLOUD_PHONES_ENABLED=1 and
-- at least one fleet host has phones_enabled = TRUE (see
-- infrastructure/cloud-phone/README.md and services/cloud_phones.rs).
--
-- Redroid needs the binder kernel module and a privileged Docker container,
-- which the unprivileged Incus containers used for cloud computers cannot
-- provide. A phone therefore runs directly on a fleet host's Docker daemon,
-- started by infrastructure/cloud-phone/run-phone.sh over SSH.
--
-- Additive only: three nullable/defaulted columns on provisioned_hosts and
-- one new table.

-- phones_enabled  : operator switch per host; default off.
-- phone_ssh_target: `user@host` the control plane SSHes to for run-phone.sh.
-- phone_mesh_ip   : the host's mesh (Tailscale/Headscale) address. adb is
--                   bound to this address only, never 0.0.0.0.
-- phone_slots     : max phones on this host (memory-bound; 0 = none).
ALTER TABLE public.provisioned_hosts
    ADD COLUMN IF NOT EXISTS phones_enabled   BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS phone_ssh_target TEXT,
    ADD COLUMN IF NOT EXISTS phone_mesh_ip    TEXT,
    ADD COLUMN IF NOT EXISTS phone_slots      INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS public.cloud_phones (
    id             TEXT PRIMARY KEY,
    user_id        TEXT NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    host_id        TEXT REFERENCES public.provisioned_hosts(id),
    container_name TEXT NOT NULL,
    label          TEXT,
    mesh_ip        TEXT,
    adb_port       INTEGER,
    -- Phone Link app (iCTRL, container port 8200) published on the same
    -- mesh address; always adb_port + 1000.
    link_port      INTEGER,
    status         TEXT NOT NULL DEFAULT 'provisioning'
        CHECK (status IN ('provisioning', 'running', 'stopped', 'error', 'deleted')),
    -- Plan id the phone is billed against. No Stripe product exists yet;
    -- pricing needs Eoj's approval before any paid plan is wired here.
    plan           TEXT NOT NULL DEFAULT 'preview',
    memory_mb      INTEGER NOT NULL DEFAULT 3072,
    last_error     TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_cloud_phones_user
    ON public.cloud_phones (user_id, created_at);

-- One live adb port per host.
CREATE UNIQUE INDEX IF NOT EXISTS idx_cloud_phones_host_port
    ON public.cloud_phones (host_id, adb_port)
    WHERE status <> 'deleted';

-- Container names are unique among live phones.
CREATE UNIQUE INDEX IF NOT EXISTS idx_cloud_phones_container
    ON public.cloud_phones (host_id, container_name)
    WHERE status <> 'deleted';
