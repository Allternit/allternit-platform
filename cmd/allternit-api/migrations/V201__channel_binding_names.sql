-- Allternit Agent Gateway: display names on channel bindings. Additive only.
--
-- The web shows "#channel in Workspace" for a bound conversation. The platform
-- ids (C0123, T0456) are not names, so the binding keeps the names the client
-- supplied when it bound the channel (or later via PATCH). Both are optional.
ALTER TABLE channel_conversation_bindings ADD COLUMN channel_name TEXT;
ALTER TABLE channel_conversation_bindings ADD COLUMN workspace_name TEXT;
