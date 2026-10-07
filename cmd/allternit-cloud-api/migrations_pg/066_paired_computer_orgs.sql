-- 066_paired_computer_orgs.sql
--
-- Allternit Factory: bots on another computer (design agreed 2026-10-07).
-- A paired computer can be used by the account that paired it AND by the
-- members of that account's organization (Clerk organization). The pairing
-- code records the organization that was active when the code was made, and
-- the computer inherits it at pairing. NULL = owner only, as before.

ALTER TABLE public.computer_pairing_codes ADD COLUMN IF NOT EXISTS organization_id text;
ALTER TABLE public.paired_computers ADD COLUMN IF NOT EXISTS organization_id text;
CREATE INDEX IF NOT EXISTS idx_paired_computers_org ON public.paired_computers (organization_id)
    WHERE organization_id IS NOT NULL;
