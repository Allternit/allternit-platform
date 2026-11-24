-- 153_computers_console.sql
--
-- Computers console (surfaces/docs/dev/computers-console-contract.mdx,
-- surfaces/docs/dev/computer-pairing.mdx), step 2.1.
--
-- paired_computers.runtime_device_id : the runtime_devices row of the same
--   machine, once pairing is unified (D7: runtime pairing is the model, a
--   paired computer links to its runtime device). NULL = not linked yet, and the
--   console then matches by owner + name (routes::computers_console).
-- provisioned_instances.location     : Orgo location chosen at create
--   (nyc, chi, lon, syd). NULL = created before this column, or Incus.
-- provisioned_instances.template     : Orgo starter template the computer
--   was created from. NULL = the default Ubuntu desktop image.
--
-- Additive only, IF NOT EXISTS everywhere: old code keeps running on the new
-- schema. No DO blocks: the test harness splits statements.

ALTER TABLE public.paired_computers ADD COLUMN IF NOT EXISTS runtime_device_id text;
CREATE INDEX IF NOT EXISTS idx_paired_computers_runtime_device
    ON public.paired_computers (runtime_device_id);

ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS location TEXT;
ALTER TABLE public.provisioned_instances ADD COLUMN IF NOT EXISTS template TEXT;
