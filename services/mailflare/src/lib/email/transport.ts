/**
 * Outbound email transport abstraction for the allternit mailflare fork.
 *
 * Two transports are supported:
 *   - cloudflare (default): uses the Cloudflare Email Service `send_email` binding.
 *   - resend: uses the Resend REST API (free tier 100 emails/day), which lets the
 *     rail work on Cloudflare's free Workers plan.
 *
 * The approval gate, queue, idempotency, and audit machinery live in send.ts; this
 * module is only the final delivery call.
 */

import type { AttachmentContent } from "@/lib/email/attachment-types";

export type TransportMessage = {
	from: string;
	to: string;
	subject: string;
	html?: string;
	text?: string;
	/** Custom headers already restricted to the send allow-list. */
	headers?: Record<string, string>;
	attachments: AttachmentContent[];
};

export type TransportDeliveryResult = {
	providerMessageId: string;
};

export function isCloudflareEmailTransport(env: CloudflareEnv): boolean {
	return (env.EMAIL_TRANSPORT ?? "cloudflare").trim().toLowerCase() !== "resend";
}

export function isResendEmailTransport(env: CloudflareEnv): boolean {
	return (env.EMAIL_TRANSPORT ?? "cloudflare").trim().toLowerCase() === "resend";
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
	const bytes = new Uint8Array(buffer);
	let binary = "";
	for (let i = 0; i < bytes.length; i++) {
		binary += String.fromCharCode(bytes[i]);
	}
	return btoa(binary);
}

async function deliverViaCloudflare(
	env: CloudflareEnv,
	message: TransportMessage,
): Promise<TransportDeliveryResult> {
	const response = await env.EMAIL.send({
		from: message.from,
		to: message.to,
		subject: message.subject,
		html: message.html,
		text: message.text,
		...(message.headers && Object.keys(message.headers).length > 0 ? { headers: message.headers } : {}),
		attachments: message.attachments.map((attachment) =>
			attachment.disposition === "inline" && attachment.contentId
				? {
						filename: attachment.filename,
						type: attachment.type,
						content: attachment.content,
						disposition: "inline" as const,
						contentId: attachment.contentId,
					}
				: {
						filename: attachment.filename,
						type: attachment.type,
						content: attachment.content,
						disposition: "attachment" as const,
					},
		),
	});
	return { providerMessageId: response.messageId };
}

type ResendAttachment = {
	filename: string;
	content: string;
};

type ResendEmailBody = {
	from: string;
	to: string;
	subject: string;
	html?: string;
	text?: string;
	headers?: Record<string, string>;
	attachments?: ResendAttachment[];
};

type ResendEmailResponse = {
	id: string;
};

async function deliverViaResend(
	env: CloudflareEnv,
	message: TransportMessage,
): Promise<TransportDeliveryResult> {
	const apiKey = env.RESEND_API_KEY?.trim();
	if (!apiKey) {
		throw new Error(
			"Email transport is set to resend but RESEND_API_KEY is not configured.",
		);
	}

	const baseUrl = (env.RESEND_API_BASE ?? "https://api.resend.com").replace(/\/$/, "");
	const body: ResendEmailBody = {
		from: message.from,
		to: message.to,
		subject: message.subject,
		html: message.html,
		text: message.text,
	};
	if (message.headers && Object.keys(message.headers).length > 0) {
		body.headers = message.headers;
	}

	if (message.attachments.length > 0) {
		body.attachments = await Promise.all(
			message.attachments.map(async (attachment) => ({
				filename: attachment.filename,
				content: arrayBufferToBase64(attachment.content),
			})),
		);
	}

	const response = await fetch(`${baseUrl}/emails`, {
		method: "POST",
		headers: {
			"Content-Type": "application/json",
			Authorization: `Bearer ${apiKey}`,
		},
		body: JSON.stringify(body),
	});

	if (!response.ok) {
		const text = await response.text();
		let detail = text;
		try {
			const parsed = JSON.parse(text) as { message?: string };
			if (parsed.message) detail = parsed.message;
		} catch {
			// keep raw text
		}
		throw new Error(`Resend send failed (${response.status}): ${detail}`);
	}

	const result = (await response.json()) as ResendEmailResponse;
	if (!result.id) {
		throw new Error("Resend response missing id");
	}
	return { providerMessageId: result.id };
}

/**
 * Deliver a message through the configured outbound transport.
 * Throws on failure; callers record status, webhooks, and audit logs.
 */
export async function deliverMessage(
	env: CloudflareEnv,
	message: TransportMessage,
): Promise<TransportDeliveryResult> {
	if (isResendEmailTransport(env)) {
		return deliverViaResend(env, message);
	}
	return deliverViaCloudflare(env, message);
}
