-- A webhook can be scoped to one mailbox (multi-tenant bot mail: each bot's
-- mailbox delivers only to its own runtime's relay address). NULL keeps the
-- old behaviour: the webhook receives events for every mailbox of its user.
ALTER TABLE `webhooks` ADD `mailbox_id` text REFERENCES mailboxes(id) ON DELETE CASCADE;
CREATE INDEX IF NOT EXISTS `webhooks_mailbox_idx` ON `webhooks` (`mailbox_id`);
