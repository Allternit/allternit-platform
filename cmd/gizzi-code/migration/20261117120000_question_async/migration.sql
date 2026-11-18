CREATE TABLE `question_async` (
	`id` text PRIMARY KEY NOT NULL,
	`session_id` text NOT NULL,
	`root_session_id` text NOT NULL,
	`questions` text NOT NULL,
	`assumption` text NOT NULL,
	`status` text NOT NULL,
	`created_at` integer NOT NULL,
	`answered_at` integer,
	`answer` text
);
--> statement-breakpoint
CREATE INDEX `question_async_root_status_idx` ON `question_async` (`root_session_id`,`status`);
--> statement-breakpoint
CREATE INDEX `question_async_status_created_idx` ON `question_async` (`status`,`created_at`);
