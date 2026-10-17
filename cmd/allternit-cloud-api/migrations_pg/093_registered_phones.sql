-- 093_registered_phones.sql
--
-- Phones 2.0 (phase 2 platform): device identity for the phone stream relay.
-- A linked phone (Android via the Desktop's adb/scrcpy lane) is registered
-- per account by the account's own allternit-api (the Desktop's gateway),
-- which authenticates with its runtime device credential. The rows carry
-- only identity + last_seen: no screen data, no input data is ever stored.
-- `online` is never persisted — the api instance's in-memory registry marks
-- a phone online while a phone-link session is attached.

CREATE TABLE IF NOT EXISTS public.registered_phones (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id text NOT NULL,
    serial text NOT NULL,
    model text,
    name text,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    UNIQUE (account_id, serial)
);

CREATE INDEX IF NOT EXISTS idx_registered_phones_account
    ON public.registered_phones (account_id);
