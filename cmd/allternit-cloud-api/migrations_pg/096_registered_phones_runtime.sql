-- 096_registered_phones_runtime.sql
--
-- Platform API /v1/phones: which runtime device (the Desktop whose phone-link
-- session serves the phone) a registered phone was last seen on, so a
-- developer call for a serial is relayed to that runtime.
--
-- Additive only: one nullable column. Rows written before this stay NULL and
-- fall back to the account's most recently seen runtime.

ALTER TABLE public.registered_phones ADD COLUMN IF NOT EXISTS runtime_id text;
