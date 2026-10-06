interface CloudflareEnv {
	DB: D1Database;
	EMAIL: SendEmail;
	BUCKET: R2Bucket;
	INBOUND_QUEUE: Queue<
		| import("./src/lib/email/inbound").InboundQueueMessage
		| import("./src/lib/email/webhooks").WebhookDeliveryQueueMessage
	>;
	OUTBOUND_QUEUE: Queue<import("./src/lib/email/send").OutboundQueueMessage>;
	ASSETS: Fetcher;
	IMAGES: ImagesBinding;
	WORKER_SELF_REFERENCE: Fetcher;
	REALTIME: DurableObjectNamespace<
		import("./src/lib/realtime/hub").RealtimeHub
	>;
	DATABASE_BACKUP_WORKFLOW: Workflow<import("./src/lib/backups/types").BackupWorkflowParams>;
	LOGIN_RATE_LIMIT?: RateLimit;
	SEND_RATE_LIMIT?: RateLimit;
	REQUIRE_SEND_APPROVAL?: string;
	EMAIL_TRANSPORT?: string;
	RESEND_API_KEY?: string;
	RESEND_API_BASE?: string;
	/** Allternit mail host for customer domains, e.g. https://mx.allternit.com/relay */
	MAIL_RELAY_URL?: string;
	/** Shared secret with the mail relay (both directions). */
	MAIL_RELAY_SECRET?: string;
	CF_TOKEN?: string;
	CF_API_KEY?: string;
	CF_EMAIL?: string;
	CF_AID?: string;
	D1_DATABASE_ID?: string;
	D1_BACKUP_TOKEN?: string;
	TURNSTILE_SECRET_KEY?: string;
	CF_EMAIL_WORKER_NAME?: string;
	GITHUB_UPDATE_TOKEN?: string;
	GITHUB_UPDATE_REF?: string;
	GITHUB_UPDATE_REPO?: string
}
