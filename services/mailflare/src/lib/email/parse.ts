import PostalMime from "postal-mime";
import { formatPostalAddress, formatPostalAddressList } from "@/lib/email/address";
import { normalizeAttachmentContent } from "@/lib/email/attachments";
import { getLatestEmailContent, htmlToReadableText } from "@/lib/email/reply-content-utils";
import type { AttachmentContent } from "@/lib/email/attachment-types";

export type ParsedEmail = {
	subject: string | null;
	text: string | null;
	html: string | null;
	messageId: string | null;
	fromAddr: string | null;
	toAddr: string | null;
	date: Date | null;
	/** Threading + loop-guard headers, for the inbound webhook payload. */
	references: string | null;
	inReplyTo: string | null;
	replyTo: string | null;
	precedence: string | null;
	autoSubmitted: string | null;
	listId: string | null;
	listUnsubscribe: string | null;
	xAutoreply: string | null;
	xAutorespond: string | null;
	xAutoResponseSuppress: string | null;
	authenticationResults: string | null;
	attachments: AttachmentContent[];
};

/**
 * First value of a header from a parsed email. postal-mime ≥2 exposes headers
 * as `{key, value}[]`; tolerate a plain record too so a shape change degrades
 * to "header absent" instead of dropping every inbound email.
 */
function headerValue(headers: unknown, name: string): string | null {
	const wanted = name.toLowerCase();
	if (Array.isArray(headers)) {
		for (const entry of headers as Array<{ key?: string; value?: unknown }>) {
			if (entry && typeof entry.key === "string" && entry.key.toLowerCase() === wanted) {
				return typeof entry.value === "string" && entry.value.length > 0 ? entry.value : null;
			}
		}
		return null;
	}
	if (headers && typeof headers === "object") {
		const record = headers as Record<string, unknown>;
		for (const [key, value] of Object.entries(record)) {
			if (key.toLowerCase() === wanted && typeof value === "string" && value.length > 0) {
				return value;
			}
		}
	}
	return null;
}

export async function parseRawMime(raw: ArrayBuffer): Promise<ParsedEmail> {
	const email = await PostalMime.parse(raw);
	const date = email.date ? new Date(email.date) : null;
	const headers = (email as { headers?: unknown }).headers;
	return {
		subject: email.subject ?? null,
		text: email.text ?? null,
		html: email.html ?? null,
		messageId: email.messageId ?? null,
		fromAddr: formatPostalAddress(email.from, null),
		toAddr: formatPostalAddressList(email.to, null),
		date: date && !Number.isNaN(date.getTime()) ? date : null,
		references: headerValue(headers, "references"),
		inReplyTo: headerValue(headers, "in-reply-to"),
		replyTo: headerValue(headers, "reply-to"),
		precedence: headerValue(headers, "precedence"),
		autoSubmitted: headerValue(headers, "auto-submitted"),
		listId: headerValue(headers, "list-id"),
		listUnsubscribe: headerValue(headers, "list-unsubscribe"),
		xAutoreply: headerValue(headers, "x-autoreply"),
		xAutorespond: headerValue(headers, "x-autorespond"),
		xAutoResponseSuppress: headerValue(headers, "x-auto-response-suppress"),
		authenticationResults: headerValue(headers, "authentication-results"),
		attachments: email.attachments.map((attachment, index) => ({
			filename: attachment.filename ?? `attachment-${index + 1}`,
			type: attachment.mimeType || "application/octet-stream",
			content: normalizeAttachmentContent(attachment.content, attachment.encoding),
			disposition: attachment.disposition === "inline" ? "inline" : "attachment",
			contentId: attachment.contentId ?? null,
		})),
	};
}

export function buildSnippet(text: string | null, html: string | null, max = 200): string {
	const source = getLatestEmailContent(text ?? htmlToReadableText(html));
	return source.replace(/\s+/g, " ").trim().slice(0, max);
}
