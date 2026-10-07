-- 081_paired_cloud_computers.sql
--
-- Allternit Factory phase 4: a cloud computer (provisioned_instances) pairs
-- itself as a paired computer so `gizzi agents up --on <computer>` can run
-- bots on it. Its allternit-api calls POST /api/v1/computers/paired/self
-- with its runtime device token; the row links back to the instance, one
-- row per instance, and is deleted when the instance is deleted.
-- NULL = a machine paired by hand with a code, as before.

ALTER TABLE public.paired_computers ADD COLUMN IF NOT EXISTS provisioned_instance_id text;
CREATE UNIQUE INDEX IF NOT EXISTS idx_paired_computers_instance
    ON public.paired_computers (provisioned_instance_id);
