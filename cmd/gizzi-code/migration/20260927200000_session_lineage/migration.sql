ALTER TABLE `session` ADD `continues_from` text;--> statement-breakpoint
ALTER TABLE `session` ADD `handoff` text;--> statement-breakpoint
CREATE INDEX `session_continues_from_idx` ON `session` (`continues_from`);
