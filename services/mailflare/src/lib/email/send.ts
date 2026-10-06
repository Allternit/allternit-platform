import { and, eq } from "drizzle-orm";
import { getDb } from "@/db";
import { messageAttachments, messageBodies, messages, outboundJobs } from "@/db/schema";
import { newId } from "@/lib/ids";
import { buildSnippet } from "@/lib/email/parse";
import { dispatchWebhooks } from "@/lib/email/webhooks";
import { deliverMessage } from "@/lib/email/transport";
import { upsertContactFromAddress } from "@/lib/contacts/service";
import { getAuthorizedSenderAddress } from "@/lib/email/sender";
import { createAuditLog } from "@/lib/mailboxes/audit";
import { storeMessageAttachments, validateAttachments } from "@/lib/email/attachments";
import type { AttachmentContent } from "@/lib/email/attachment-types";

export type SendEmailInput = {
	userId: string;
	from: string;
	to: string;
	subject: string;
	html?: string;
	text?: string;
	mailboxId: string;
	/** Custom headers (already allow-list validated) passed to the provider as-is. */
	headers?: Record<string, string>;
	attachments?: AttachmentContent[];
};

export type SendEmailOptions = {
	/** Idempotency-Key header value; replays return the original job instead of duplicating. */
	idempotencyKey?: string | null;
	/**
	 * Deliver without the approval gate. The route only sets this for
	 * admin-scope keys; mailbox-scoped keys always go through approval.
	 */
	skipApproval?: boolean;
};

export type SendEmailResult = {
	messageId: string;
	jobId: string;
	status: string;
	idempotentReplay: boolean;
};

/**
 * This fork gates outbound mail behind human approval by default — that is its whole
 * point as the allternit "agent email" transport. Set REQUIRE_SEND_APPROVAL=false to
 * restore the upstream synchronous send behavior.
 */
export function isSendApprovalRequired(env: CloudflareEnv): boolean {
	return (env.REQUIRE_SEND_APPROVAL ?? "true").trim().toLowerCase() !== "false";
}


type DeliveryParams = {
	jobId: string;
	messageId: string;
	userId: string;
	mailboxId: string;
	from: string;
	to: string;
	subject: string;
	html?: string;
	text?: string;
	headers?: Record<string, string>;
	attachments: AttachmentContent[];
};

/**
 * Deliver an already-persisted outbound message via the email provider and record the
 * outcome (job/message status, providerMessageId, webhooks, audit log). Throws after
 * recording when the provider send fails; callers decide whether to propagate.
 */
async function deliverOutboundMessage(env: CloudflareEnv, params: DeliveryParams): Promise<void> {
	const db = getDb(env);
	try {
		const response = await deliverMessage(env, {
			from: params.from,
			to: params.to,
			subject: params.subject,
			html: params.html,
			text: params.text,
			headers: params.headers,
			attachments: params.attachments,
		});

		await db
			.update(messages)
			.set({ status: "sent", providerMessageId: response.providerMessageId })
			.where(eq(messages.id, params.messageId));
		await db
			.update(outboundJobs)
			.set({ status: "sent", updatedAt: new Date() })
			.where(eq(outboundJobs.id, params.jobId));

		await dispatchWebhooks(env, params.userId, "message.outbound", {
			messageId: params.messageId,
			providerMessageId: response.providerMessageId,
			to: params.to,
		}, params.mailboxId);
		await createAuditLog(env, {
			actorUserId: params.userId,
			mailboxId: params.mailboxId,
			messageId: params.messageId,
			action: "email.send",
			metadata: { to: params.to, subject: params.subject },
		});
	} catch (err) {
		const error = err instanceof Error ? err.message : "Send failed";
		await db.update(messages).set({ status: "failed" }).where(eq(messages.id, params.messageId));
		await db
			.update(outboundJobs)
			.set({ status: "failed", error, updatedAt: new Date() })
			.where(eq(outboundJobs.id, params.jobId));
		await dispatchWebhooks(env, params.userId, "message.failed", {
			messageId: params.messageId,
			error,
		}, params.mailboxId);
		throw err;
	}
}

export async function sendEmail(
	env: CloudflareEnv,
	input: SendEmailInput,
	options?: SendEmailOptions,
): Promise<SendEmailResult> {
	const db = getDb(env);
	const idempotencyKey = options?.idempotencyKey?.trim() || null;

	if (idempotencyKey) {
		const [existing] = await db
			.select()
			.from(outboundJobs)
			.where(and(eq(outboundJobs.userId, input.userId), eq(outboundJobs.idempotencyKey, idempotencyKey)))
			.limit(1);
		if (existing?.messageId) {
			return {
				messageId: existing.messageId,
				jobId: existing.id,
				status: existing.status,
				idempotentReplay: true,
			};
		}
	}

	const sender = await getAuthorizedSenderAddress(env, input);
	const attachments = input.attachments ?? [];
	validateAttachments(attachments);
	await upsertContactFromAddress(env, {
		userId: input.userId,
		address: input.to,
		source: "outbound",
	});
	const messageId = newId("msg");
	const snippet = buildSnippet(input.text ?? null, input.html ?? null);
	const approvalRequired = isSendApprovalRequired(env) && options?.skipApproval !== true;

	await db.insert(messages).values({
		id: messageId,
		userId: input.userId,
		mailboxId: sender.mailboxId,
		direction: "outbound",
		fromAddr: sender.fromAddr,
		toAddr: input.to,
		subject: input.subject,
		snippet,
		status: approvalRequired ? "pending_approval" : "queued",
	});

	await db.insert(messageBodies).values({
		id: newId(),
		messageId,
		textBody: input.text ?? null,
		htmlBody: input.html ?? null,
	});
	try {
		await storeMessageAttachments(env, messageId, attachments);
	} catch (error) {
		await db.delete(messages).where(eq(messages.id, messageId));
		throw error;
	}

	const jobId = newId("job");
	await db.insert(outboundJobs).values({
		id: jobId,
		userId: input.userId,
		messageId,
		status: approvalRequired ? "pending_approval" : "queued",
		idempotencyKey,
		payload: JSON.stringify({
			...input,
			from: sender.fromAddr,
			mailboxId: sender.mailboxId,
			attachments: attachments.map(({ content: _content, ...attachment }) => attachment),
		}),
	});

	if (approvalRequired) {
		// Gated: nothing is sent and nothing is enqueued until the job is approved
		// via POST /api/v1/outbound/[id]/approve.
		await createAuditLog(env, {
			actorUserId: input.userId,
			mailboxId: sender.mailboxId,
			messageId,
			action: "email.send.pending_approval",
			metadata: { to: input.to, subject: input.subject, jobId },
		});
		return { messageId, jobId, status: "pending_approval", idempotentReplay: false };
	}

	await deliverOutboundMessage(env, {
		jobId,
		messageId,
		userId: input.userId,
		mailboxId: sender.mailboxId,
		from: sender.fromAddr,
		to: input.to,
		subject: input.subject,
		html: input.html,
		text: input.text,
		headers: input.headers,
		attachments,
	});
	return { messageId, jobId, status: "sent", idempotentReplay: false };
}

export type OutboundQueueMessage = { jobId: string };

type StoredOutboundPayload = {
	userId: string;
	from: string;
	to: string;
	subject: string;
	html?: string;
	text?: string;
	mailboxId: string;
	headers?: Record<string, string>;
};

/**
 * Queue consumer entrypoint for outbound jobs. Loads the job, re-reads attachment
 * content from R2 (the stored payload only carries metadata), and delivers via the
 * provider. Jobs in any state other than "queued" (pending_approval, sent, failed)
 * are acknowledged without action. Failures are recorded, not retried, matching the
 * synchronous path's semantics.
 */
export async function processOutboundQueue(
	env: CloudflareEnv,
	payload: OutboundQueueMessage,
): Promise<void> {
	const db = getDb(env);
	const [job] = await db.select().from(outboundJobs).where(eq(outboundJobs.id, payload.jobId)).limit(1);
	if (!job) {
		console.warn(`Outbound job not found: ${payload.jobId}`);
		return;
	}
	if (job.status !== "queued") return;
	if (!job.messageId) {
		await db
			.update(outboundJobs)
			.set({ status: "failed", error: "Job has no message", updatedAt: new Date() })
			.where(eq(outboundJobs.id, job.id));
		return;
	}

	const stored = JSON.parse(job.payload) as StoredOutboundPayload;

	const attachmentRows = await db
		.select()
		.from(messageAttachments)
		.where(eq(messageAttachments.messageId, job.messageId));
	const attachments: AttachmentContent[] = await Promise.all(
		attachmentRows.map(async (row) => {
			const object = await env.BUCKET.get(row.r2Key);
			if (!object) throw new Error(`Missing attachment object: ${row.r2Key}`);
			return {
				filename: row.filename,
				type: row.contentType,
				content: await object.arrayBuffer(),
				disposition: row.disposition,
				contentId: row.contentId,
			};
		}),
	);

	try {
		await deliverOutboundMessage(env, {
			jobId: job.id,
			messageId: job.messageId,
			userId: job.userId,
			mailboxId: stored.mailboxId,
			from: stored.from,
			to: stored.to,
			subject: stored.subject,
			html: stored.html,
			text: stored.text,
			headers: stored.headers,
			attachments,
		});
	} catch (err) {
		// Outcome already recorded by deliverOutboundMessage; ack instead of retrying.
		console.error(`Outbound job ${job.id} failed`, err);
	}
}

export type OutboundJobReview = {
	job: typeof outboundJobs.$inferSelect;
	message: typeof messages.$inferSelect;
};

/** Load a job with its message for the approve/reject routes. */
export async function getOutboundJobForReview(
	env: CloudflareEnv,
	jobId: string,
): Promise<OutboundJobReview | null> {
	const db = getDb(env);
	const [job] = await db.select().from(outboundJobs).where(eq(outboundJobs.id, jobId)).limit(1);
	if (!job?.messageId) return null;
	const [message] = await db.select().from(messages).where(eq(messages.id, job.messageId)).limit(1);
	if (!message) return null;
	return { job, message };
}

/**
 * Approve a pending_approval job: mark it (and its message) queued and hand it to the
 * outbound queue. The queue consumer performs the actual provider send.
 */
export async function approveOutboundJob(env: CloudflareEnv, review: OutboundJobReview): Promise<void> {
	const db = getDb(env);
	const now = new Date();
	await db
		.update(outboundJobs)
		.set({ status: "queued", updatedAt: now })
		.where(eq(outboundJobs.id, review.job.id));
	await db.update(messages).set({ status: "queued" }).where(eq(messages.id, review.message.id));
	await env.OUTBOUND_QUEUE.send({ jobId: review.job.id } satisfies OutboundQueueMessage);
	await createAuditLog(env, {
		actorUserId: review.job.userId,
		mailboxId: review.message.mailboxId,
		messageId: review.message.id,
		action: "email.send.approved",
		metadata: { jobId: review.job.id, to: review.message.toAddr, subject: review.message.subject },
	});
}

/** Reject a pending_approval job: mark job and message failed without sending. */
export async function rejectOutboundJob(env: CloudflareEnv, review: OutboundJobReview): Promise<void> {
	const db = getDb(env);
	await db
		.update(outboundJobs)
		.set({ status: "failed", error: "rejected", updatedAt: new Date() })
		.where(eq(outboundJobs.id, review.job.id));
	await db.update(messages).set({ status: "failed" }).where(eq(messages.id, review.message.id));
	await createAuditLog(env, {
		actorUserId: review.job.userId,
		mailboxId: review.message.mailboxId,
		messageId: review.message.id,
		action: "email.send.rejected",
		metadata: { jobId: review.job.id, to: review.message.toAddr, subject: review.message.subject },
	});
}
