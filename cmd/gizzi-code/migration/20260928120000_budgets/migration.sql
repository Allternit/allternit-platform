CREATE TABLE `budget` (
	`scope` text NOT NULL,
	`target_id` text NOT NULL,
	`limit_usd` real NOT NULL,
	`period` text NOT NULL DEFAULT 'month',
	`time_updated` integer NOT NULL,
	PRIMARY KEY(`scope`, `target_id`)
);
