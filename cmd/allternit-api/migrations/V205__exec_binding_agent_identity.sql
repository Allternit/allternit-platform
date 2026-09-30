-- Allternit Agent Gateway: the vendor agent's own name and avatar on the bot's
-- execution binding. Additive only.
--
-- A vendor-bound bot shows the avatar it has on the vendor's platform (a
-- ChatGPT dot's face, a Grok Bot bot's picture). The binding keeps what
-- discovery reported: an https URL, or an inline data:image URI when the
-- adapter could only read the image while signed in to the vendor.
ALTER TABLE bot_execution_bindings ADD COLUMN external_agent_name TEXT;
ALTER TABLE bot_execution_bindings ADD COLUMN external_agent_avatar TEXT;
