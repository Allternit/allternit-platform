-- 160_computer_whole_disk.sql
--
-- Computers console step 2.11: whole-disk file roots on servers
-- (surfaces/docs/dev/computers-console-contract.mdx, "Whole-disk files").
-- 159 is 159_computer_machine_identity.sql.
--
-- One row per machine the owner turned the setting on for (or on and then
-- off). Default when there is no row: OFF. The console Files tab then sees
-- only the Files drive folder (~/Allternit and the other Files drive roots),
-- exactly as before this step.
--
-- machine_key  : the console machine key (step 2.10: `mid:<id>`, hostname
--                key, `cloud:<id>`, `id:<id>`), so the setting covers every
--                alias of the machine.
-- runtime_ids  : the machine's runtime devices when the owner turned it on.
--                The relay checks the runtime it is about to call against
--                this list, so a device paired later (or a re-pair) starts
--                OFF until the owner turns it on again (fail closed).
--
-- Additive only, IF NOT EXISTS everywhere, no back-fill. No DO blocks: the
-- test harness splits statements. Account erase deletes the owner's rows.

CREATE TABLE IF NOT EXISTS public.computer_whole_disk (
    user_id TEXT NOT NULL,
    machine_key TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT false,
    runtime_ids TEXT[] NOT NULL DEFAULT '{}',
    enabled_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, machine_key)
);

CREATE INDEX IF NOT EXISTS idx_computer_whole_disk_enabled
    ON public.computer_whole_disk (user_id) WHERE enabled;
