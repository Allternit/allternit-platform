-- 159_computer_machine_identity.sql
--
-- Computers console step 2.10, pairing unification
-- (surfaces/docs/dev/computer-pairing.mdx). 158 is 158_computer_snippets.sql.
--
-- One console row per machine. The machine key is the one the active-machine
-- cap already uses (services::quota_service::machine_key_for): `mid:<id>` from
-- ~/.allternit/machine-id, which Desktop, `gizzi pair`, `ao fabric pair` and
-- `allternit computer pair` all send, else lower(hostname minus .local)|platform.
-- The console groups rows by that key when it reads (routes::computers_console);
-- nothing is merged or copied in the database, so dropping these is a full undo.
--
-- paired_computers.machine_id / hostname / platform : what a code-paired
--   machine sent at redeem time, so it groups with its runtime devices.
--   NULL for rows paired before this (they still group by the stored
--   runtime_device_id link, or by owner + name).
-- computer_machine_names : a name the owner gave a machine in the console
--   (PATCH /api/v1/computers/paired/:id). Keyed by machine key, so it survives
--   re-pairing and covers every alias of the machine. Owner-only.
--
-- Additive only, IF NOT EXISTS everywhere, no back-fill. No DO blocks: the
-- test harness splits statements.

-- runtime_devices.machine_id exists since migration 119; repeated here so a
-- schema built from only the console migrations (tests) has it too.
ALTER TABLE public.runtime_devices ADD COLUMN IF NOT EXISTS machine_id TEXT;
ALTER TABLE public.paired_computers ADD COLUMN IF NOT EXISTS machine_id TEXT;
ALTER TABLE public.paired_computers ADD COLUMN IF NOT EXISTS hostname TEXT;
ALTER TABLE public.paired_computers ADD COLUMN IF NOT EXISTS platform TEXT;
CREATE INDEX IF NOT EXISTS idx_paired_computers_user_machine
    ON public.paired_computers (user_id, machine_id) WHERE machine_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS public.computer_machine_names (
    user_id TEXT NOT NULL,
    machine_key TEXT NOT NULL,
    name TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, machine_key)
);
