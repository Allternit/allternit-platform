-- "Write in my style": when the assistant drafts something the user will send
-- as themselves (emails, messages, posts), it writes in the user's own style.
-- On by default. `writing_style` is a short plain-words profile built by
-- POST /agent-preferences/writing-style/learn (no model call) or edited by
-- hand; agent_preferences_routes.rs composes it into agent-chat system
-- instructions and the managed STYLE.md. Lives on the per-user preferences
-- row (V31, extended by V197) so the chat bridge reads it with the rest.
ALTER TABLE user_agent_preferences ADD COLUMN write_in_my_style INTEGER NOT NULL DEFAULT 1;
ALTER TABLE user_agent_preferences ADD COLUMN writing_style TEXT NOT NULL DEFAULT '';
ALTER TABLE user_agent_preferences ADD COLUMN writing_style_learned_at TEXT;
