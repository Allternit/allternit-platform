-- Settings → Profile: who the user is, for every agent and model. Lives on
-- the per-user preferences row (V31) so the chat bridge reads it in the same
-- query as the response style and composes an "About the user" layer.
ALTER TABLE user_agent_preferences ADD COLUMN full_name TEXT NOT NULL DEFAULT '';
ALTER TABLE user_agent_preferences ADD COLUMN preferred_name TEXT NOT NULL DEFAULT '';
ALTER TABLE user_agent_preferences ADD COLUMN occupation TEXT NOT NULL DEFAULT '';
ALTER TABLE user_agent_preferences ADD COLUMN personal_preferences TEXT NOT NULL DEFAULT '';
