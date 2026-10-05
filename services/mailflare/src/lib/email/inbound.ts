import { eq } from "drizzle-orm";
import { getDb } from "@/db";
import { messageBodies, messages } from "@/db/schema";
import { newId } from "@/lib/ids";
import { buildSnippet, parseRawMime } from "@/lib/email/parse";
import { resolveInboundAddress, resolveInboxRuleDestination } from "@/lib/email/routing";
import { dispatchWebhooks } from "@/lib/email/webhooks";
import { getMessageContactNames, upsertContactFromAddress } from "@/lib/contacts/service";
import { formatEmailAddress, getEmailAddress } from "@/lib/email/address";
import { getMailboxAccessLevel } from "@/lib/mailboxes/access";
import { listMessageAttachments, storeMessageAttachments } from "@/lib/email/attachments";
import { getUnsubscribeUrlFromRawR2Key } from "@/lib/email/unsubscribe";
import type { SessionUser } from "@/lib/auth/types";
import {
	getMailboxNotificationUserIds,
	notifyUsersOfNewMessage,
} from "@/lib/realtime/utils";

export type InboundQueueMessage = {
	from: string;
	to: string;
	rawR2Key: string;
	headers?: Record<string, string>;
};

export async function processInboundMessage(
	env: CloudflareEnv,
	payload: InboundQueueMessage,
): Promise<void> {
	const db = getDb(env);
	const decision = await resolveInboundAddress(db, payload.to);

	if (!decision) {
		console.warn(`No routing for inbound address: ${payload.to}`);
		return;
	}

	if (decision.action === "reject") {
		console.warn(`Rejected inbound: ${payload.to}`);
		return;
	}

	if (decision.action === "forward" && decision.forwardTo) {
		console.info(`Forward ${payload.to} -> ${decision.forwardTo}`);
		return;
	}

	if (!decision.mailbox) return;

	const raw = await env.BUCKET.get(payload.rawR2Key);
	if (!raw) {
		console.error(`Missing R2 object: ${payload.rawR2Key}`);
		return;
	}

	const buffer = await raw.arrayBuffer();
	const parsed = await parseRawMime(buffer);
	const messageId = newId("msg");
	const snippet = buildSnippet(parsed.text, parsed.html);
	const mailboxAddress = `${decision.mailbox.localPart}@${decision.mailbox.hostname}`;
	const mailboxHeader = formatEmailAddress(mailboxAddress, decision.mailbox.displayName ?? decision.mailbox.localPart);
	const toAddr = parsed.toAddr && getEmailAddress(parsed.toAddr).toLowerCase() !== mailboxAddress.toLowerCase()
		? parsed.toAddr
		: mailboxHeader;
	const fromAddr = parsed.fromAddr ?? payload.from;
	const destination = await resolveInboxRuleDestination(db, {
		mailboxId: decision.mailbox.mailboxId,
		toAddress: toAddr,
		fromAddress: fromAddr,
		subject: parsed.subject,
		content: [parsed.text, parsed.html, snippet].filter(Boolean).join(" "),
	});
	await upsertContactFromAddress(env, {
		userId: decision.mailbox.userId,
		address: fromAddr,
		source: "inbound",
	});

	try {
		await db.insert(messages).values({
			id: messageId,
			userId: decision.mailbox.userId,
			mailboxId: decision.mailbox.mailboxId,
			folderId: destination.folderId,
			direction: "inbound",
			providerMessageId: parsed.messageId,
			fromAddr,
			toAddr,
			subject: parsed.subject,
			snippet,
			status: destination.status,
			threadId: parsed.messageId,
		});

		await db.insert(messageBodies).values({
			id: newId(),
			messageId,
			textBody: parsed.text,
			htmlBody: parsed.html,
			rawR2Key: payload.rawR2Key,
		});
		await storeMessageAttachments(env, messageId, parsed.attachments, { validate: false });
	} catch (error) {
		await db.delete(messages).where(eq(messages.id, messageId));
		throw error;
	}

	const notificationUserIds = await getMailboxNotificationUserIds(
		env,
		decision.mailbox.mailboxId,
		decision.mailbox.userId,
	);
	await notifyUsersOfNewMessage(env, notificationUserIds, {
		type: "new_message",
		messageId,
		mailboxId: decision.mailbox.mailboxId,
		from: fromAddr,
		subject: parsed.subject,
	});
	await dispatchWebhooks(env, decision.mailbox.userId, "message.inbound", {
		messageId,
		mailboxId: decision.mailbox.mailboxId,
		// The mailbox the mail was delivered to (the To header can differ for Cc/Bcc).
		mailbox: mailboxAddress,
		from: fromAddr,
		to: toAddr,
		subject: parsed.subject,
		snippet,
		textBody: parsed.text ? parsed.text.slice(0, 4000) : null,
		headers: {
			from: fromAddr,
			to: toAddr,
			subject: parsed.subject,
			messageId: parsed.messageId,
			date: parsed.date ? parsed.date.toISOString() : null,
			references: parsed.references,
			inReplyTo: parsed.inReplyTo,
			replyTo: parsed.replyTo,
			precedence: parsed.precedence,
			autoSubmitted: parsed.autoSubmitted,
			listId: parsed.listId,
			listUnsubscribe: parsed.listUnsubscribe,
			xAutoreply: parsed.xAutoreply,
			xAutorespond: parsed.xAutorespond,
			xAutoResponseSuppress: parsed.xAutoResponseSuppress,
		},
		// SPF/DKIM/DMARC verdicts from the receiving platform; bots must not
		// reply to a forged sender, so consumers can gate on dmarc=fail.
		authResults: parsed.authenticationResults,
	}, decision.mailbox.mailboxId);
}

export async function storeRawToR2(
	env: CloudflareEnv,
	from: string,
	to: string,
	raw: ReadableStream<Uint8Array>,
): Promise<string> {
	const key = `inbound/${Date.now()}-${newId()}.eml`;
	const buffer = await new Response(raw).arrayBuffer();
	await env.BUCKET.put(key, buffer, {
		httpMetadata: { contentType: "message/rfc822" },
		customMetadata: { from, to },
	});
	return key;
}

export async function getMessageWithBody(env: CloudflareEnv, userId: string, messageId: string) {
	const db = getDb(env);
	const [message] = await db
		.select()
		.from(messages)
		.where(eq(messages.id, messageId))
		.limit(1);
	if (!message || message.userId !== userId) return null;
	const [body] = await db
		.select()
		.from(messageBodies)
		.where(eq(messageBodies.messageId, messageId))
		.limit(1);
	const contactNames = await getMessageContactNames(env, userId, message.fromAddr, message.toAddr);
	const attachments = await listMessageAttachments(env, messageId);
	const unsubscribeUrl = await getUnsubscribeUrlFromRawR2Key(env, body?.rawR2Key ?? null);
	return { message: { ...message, ...contactNames }, body, attachments, unsubscribeUrl };
}

export async function getMessageWithBodyForUser(env: CloudflareEnv, user: SessionUser, messageId: string) {
	const db = getDb(env);
	const [message] = await db.select().from(messages).where(eq(messages.id, messageId)).limit(1);
	if (!message?.mailboxId) return null;
	const access = await getMailboxAccessLevel(db, user, message.mailboxId);
	if (!access?.canRead) return null;
	const [body] = await db
		.select()
		.from(messageBodies)
		.where(eq(messageBodies.messageId, messageId))
		.limit(1);
	const contactNames = await getMessageContactNames(env, user.id, message.fromAddr, message.toAddr);
	const attachments = await listMessageAttachments(env, messageId);
	const unsubscribeUrl = await getUnsubscribeUrlFromRawR2Key(env, body?.rawR2Key ?? null);
	return { message: { ...message, ...contactNames }, body, attachments, unsubscribeUrl };
}
