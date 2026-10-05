import { eq } from "drizzle-orm";
import { getDb } from "@/db";
import { webhookDeliveries, webhooks } from "@/db/schema";
import { newId } from "@/lib/ids";

export type WebhookEventType = "message.inbound" | "message.outbound" | "message.failed";

export type WebhookDeliveryQueueMessage = {
	type: "webhook_delivery";
	deliveryId: string;
};

const MAX_DELIVERY_ATTEMPTS = 5;
const BASE_RETRY_DELAY_SECONDS = 30;

/** Exponential backoff between delivery attempts: 30s, 60s, 120s, 240s. */
function retryDelaySeconds(attempt: number): number {
	return BASE_RETRY_DELAY_SECONDS * 2 ** (attempt - 1);
}

/**
 * Fan an event out to the user's enabled webhooks. Each delivery is persisted in
 * webhook_deliveries and handed to the inbound queue, which performs the actual HTTP
 * POST (with retries) in processWebhookDelivery — so deliveries survive restarts and
 * never block the caller.
 */
export async function dispatchWebhooks(
	env: CloudflareEnv,
	userId: string,
	eventType: WebhookEventType,
	payload: Record<string, unknown>,
	mailboxId?: string | null,
): Promise<void> {
	const db = getDb(env);
	const hooks = await db.select().from(webhooks).where(eq(webhooks.userId, userId));

	for (const hook of hooks) {
		if (!hook.enabled) continue;
		// A mailbox-scoped webhook gets only its own mailbox's events.
		if (hook.mailboxId && hook.mailboxId !== mailboxId) continue;
		let events: string[] = [];
		try {
			events = JSON.parse(hook.events) as string[];
		} catch {
			continue;
		}
		if (!events.includes(eventType)) continue;

		const deliveryId = newId();
		const body = JSON.stringify({ type: eventType, data: payload });
		await db.insert(webhookDeliveries).values({
			id: deliveryId,
			webhookId: hook.id,
			eventType,
			payload: body,
			status: "pending",
			attempts: 0,
		});
		await env.INBOUND_QUEUE.send({
			type: "webhook_delivery",
			deliveryId,
		} satisfies WebhookDeliveryQueueMessage);
	}
}

/**
 * Queue consumer entrypoint for webhook deliveries. On failure (non-2xx or network
 * error) the attempt is recorded and the delivery is re-enqueued with exponential
 * backoff, up to MAX_DELIVERY_ATTEMPTS total attempts.
 */
export async function processWebhookDelivery(
	env: CloudflareEnv,
	payload: WebhookDeliveryQueueMessage,
): Promise<void> {
	const db = getDb(env);
	const [delivery] = await db
		.select()
		.from(webhookDeliveries)
		.where(eq(webhookDeliveries.id, payload.deliveryId))
		.limit(1);
	if (!delivery) {
		console.warn(`Webhook delivery not found: ${payload.deliveryId}`);
		return;
	}
	if (delivery.status !== "pending") return;

	const [hook] = await db.select().from(webhooks).where(eq(webhooks.id, delivery.webhookId)).limit(1);
	if (!hook || !hook.enabled) {
		await db
			.update(webhookDeliveries)
			.set({ status: "failed" })
			.where(eq(webhookDeliveries.id, delivery.id));
		return;
	}

	const attempts = delivery.attempts + 1;
	let delivered = false;
	try {
		const signature = await signPayload(hook.secret, delivery.payload);
		const res = await fetch(hook.url, {
			method: "POST",
			redirect: "manual",
			signal: AbortSignal.timeout(10_000),
			headers: {
				"Content-Type": "application/json",
				"X-Email-Platform-Signature": signature,
				"X-Email-Platform-Event": delivery.eventType,
			},
			body: delivery.payload,
		});
		delivered = res.ok;
		if (!res.ok) {
			console.warn(`Webhook delivery ${delivery.id} got HTTP ${res.status}`);
		}
	} catch (error) {
		console.warn(`Webhook delivery ${delivery.id} failed`, error);
	}

	if (delivered) {
		await db
			.update(webhookDeliveries)
			.set({ status: "delivered", attempts })
			.where(eq(webhookDeliveries.id, delivery.id));
		return;
	}

	if (attempts >= MAX_DELIVERY_ATTEMPTS) {
		await db
			.update(webhookDeliveries)
			.set({ status: "failed", attempts })
			.where(eq(webhookDeliveries.id, delivery.id));
		return;
	}

	await db
		.update(webhookDeliveries)
		.set({ status: "pending", attempts })
		.where(eq(webhookDeliveries.id, delivery.id));
	await env.INBOUND_QUEUE.send(
		{ type: "webhook_delivery", deliveryId: delivery.id } satisfies WebhookDeliveryQueueMessage,
		{ delaySeconds: retryDelaySeconds(attempts) },
	);
}

async function signPayload(secret: string, body: string): Promise<string> {
	const key = await crypto.subtle.importKey(
		"raw",
		new TextEncoder().encode(secret),
		{ name: "HMAC", hash: "SHA-256" },
		false,
		["sign"],
	);
	const sig = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(body));
	return Array.from(new Uint8Array(sig))
		.map((b) => b.toString(16).padStart(2, "0"))
		.join("");
}
