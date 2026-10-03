-- V221: where a vendor (browser-session) account's browser runs.
--
-- `this_device` (default, today's behaviour): the owner's Desktop. `cloud_computer`:
-- a paired cloud-computer runtime (`host_runtime_id` = its runtime-device id), which
-- keeps the vendor bots online while the Mac sleeps. Moving host never copies
-- cookies: it sets host_state = 'needs_sign_in' and the person signs in on the new host.
--
--   host_state: ready | needs_sign_in | signing_in
--   host_remote_account_id: the account's id inside the cloud computer's subscription gateway
ALTER TABLE provider_account_bindings ADD COLUMN host_kind TEXT NOT NULL DEFAULT 'this_device';
ALTER TABLE provider_account_bindings ADD COLUMN host_runtime_id TEXT;
ALTER TABLE provider_account_bindings ADD COLUMN host_state TEXT NOT NULL DEFAULT 'ready';
ALTER TABLE provider_account_bindings ADD COLUMN host_remote_account_id TEXT;
ALTER TABLE provider_account_bindings ADD COLUMN host_last_seen_at TEXT;
ALTER TABLE provider_account_bindings ADD COLUMN host_changed_at TEXT;
