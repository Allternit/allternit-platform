-- V216: Slack moves onto the shared Allternit app — connect once, switch any
-- number of bots on per connection — the same model as the other messaging
-- connectors (V211). Legacy per-channel bot bindings (slack_channel_bots,
-- served by slack_webhook_routes.rs) become bot memberships on a Slack
-- connection of the same owner, so the new shared-app routing
-- (channel_slack_app.rs) and the Messaging UI see them. The old table and
-- routes keep working; the migrated connection has no secret_ref because a
-- legacy (custom-app) install holds no shared-app token — sends for it keep
-- using the env-token transport until the owner connects the shared app.

-- One Slack connection per owner that has legacy channel bindings (skipped
-- when the owner already connected Slack). external_account_id 'legacy' marks
-- rows born of this migration.
INSERT INTO provider_account_bindings
    (id, owner, vendor, auth_type, external_account_id, display_name, scopes_json, state, created_at, updated_at)
SELECT 'acct_slack_legacy_' || lower(hex(randomblob(8))),
       user_id, 'slack', 'channel_oauth', 'legacy', 'Slack (legacy channels)',
       '["messages"]', 'CONNECTED', MIN(created_at), MIN(created_at)
FROM slack_channel_bots
WHERE user_id NOT IN (
    SELECT owner FROM provider_account_bindings
    WHERE vendor = 'slack' AND auth_type = 'channel_oauth'
)
GROUP BY user_id;

-- The bound bots become members of that connection; the bot of the owner's
-- earliest binding answers new conversations (is_default), the rest follow
-- mentions. Rows that already exist (a bot bound to two legacy channels) are
-- kept once by INSERT OR IGNORE and their earliest binding wins the default.
INSERT OR IGNORE INTO channel_account_bots (account_id, bot_id, owner, is_default, created_at)
SELECT a.id, s.bot_id, s.user_id,
    CASE WHEN s.created_at = (
        SELECT MIN(s2.created_at) FROM slack_channel_bots s2 WHERE s2.user_id = s.user_id
    ) THEN 1 ELSE 0 END,
    s.created_at
FROM slack_channel_bots s
JOIN provider_account_bindings a
  ON a.owner = s.user_id AND a.vendor = 'slack'
 AND a.auth_type = 'channel_oauth' AND a.external_account_id = 'legacy';
