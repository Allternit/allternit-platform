CREATE TABLE `message_branch` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`anchor_message_id` text NOT NULL,
	`created_at` integer NOT NULL,
	`payload` text NOT NULL,
	FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `message_branch_session_idx` ON `message_branch` (`session_id`,`created_at`);
