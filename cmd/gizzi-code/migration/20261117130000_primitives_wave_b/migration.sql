ALTER TABLE `session` ADD `task_name` text;
--> statement-breakpoint
CREATE INDEX `session_task_name_idx` ON `session` (`parent_id`,`task_name`);
--> statement-breakpoint
ALTER TABLE `todo` ADD `item_id` text;
--> statement-breakpoint
ALTER TABLE `todo` ADD `active_form` text;
--> statement-breakpoint
ALTER TABLE `todo` ADD `blocked_by` text;
