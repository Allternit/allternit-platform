import { z } from "zod";
import { ALLOWED_OUTBOUND_HEADERS, normalizeOutboundHeaders } from "@/lib/email/custom-headers";

// The only custom headers a caller may set on an outbound message. The allow-list
// lives in custom-headers.ts so the node:test suite can pin it without a build step.
const outboundHeaderSchemas = Object.fromEntries(
	ALLOWED_OUTBOUND_HEADERS.map((name) => [name, z.string().max(1000).optional()]),
) as Record<string, z.ZodOptional<z.ZodString>>;

export const sendEmailSchema = z
	.object({
		from: z.string().min(3).max(500),
		to: z.string().min(3).max(500),
		subject: z.string().min(1).max(500),
		html: z.string().max(2 * 1024 * 1024).optional(),
		text: z.string().max(2 * 1024 * 1024).optional(),
		mailboxId: z.string().min(1).max(200),
		// Reply threading / auto-reply markers. Only the four sanctioned headers;
		// anything else fails the strict parse below with a 400.
		headers: z.object(outboundHeaderSchemas).strict().optional(),
		// Admin-scope keys only: deliver one message without the approval gate.
		// Mailbox-scoped keys are ignored here (never rejected, so older callers
		// keep working) and always go through approval.
		skipApproval: z.boolean().optional(),
		attachments: z
			.array(
				z.object({
						filename: z.string().min(1).max(255),
						type: z.string().min(1).max(255).default("application/octet-stream"),
						contentBase64: z.string().min(1).max(14 * 1024 * 1024),
				}),
			)
			.max(10)
			.optional(),
	})
	.superRefine((value, ctx) => {
		if (value.headers === undefined) return;
		const normalized = normalizeOutboundHeaders(value.headers);
		if (!normalized.ok) {
			ctx.addIssue({
				code: "custom",
				path: ["headers"],
				message: `Disallowed outbound header(s): ${normalized.rejected.join(", ")}`,
			});
		}
	});

export const registerSchema = z.object({
	email: z.string().email(),
	password: z.string().min(8),
	name: z.string().min(1),
});

export const firstRunRegisterSchema = z.object({
	domain: z.string().min(3),
	username: z.string().min(1).max(64).regex(/^[a-zA-Z0-9._%+-]+$/),
	password: z.string().min(8),
	resetEmail: z.string().email(),
});

export const primaryDomainRegisterSchema = z.object({
	username: z.string().min(1).max(64).regex(/^[a-zA-Z0-9._%+-]+$/),
	password: z.string().min(8),
	resetEmail: z.string().email(),
});

export const setupDomainSchema = z.object({
	hostname: z.string().min(3),
});

export const addDomainSchema = z.object({
	hostname: z.string().min(3),
	enableRouting: z.boolean().optional(),
	enableSending: z.boolean().optional(),
});

export const loginSchema = z.object({
	email: z.string().email(),
	password: z.string().min(1),
});

export const domainSchema = z.object({
	hostname: z.string().min(3),
});

export const mailboxSchema = z.object({
	domainId: z.string().min(1),
	localPart: z.string().min(1).max(64),
	displayName: z.string().optional(),
	type: z.enum(["personal", "shared"]).optional(),
});

export const createAccountSchema = z.object({
	domainId: z.string().min(1),
	username: z.string().min(1).max(64).regex(/^[a-zA-Z0-9._%+-]+$/),
	password: z.string().min(8),
	name: z.string().trim().min(1).max(100).optional(),
	resetEmail: z.preprocess(
		(value) => (typeof value === "string" ? value.trim() : value),
		z.string().email().or(z.literal("")).optional().transform((value) => value || null),
	),
});

export const updateAccountSchema = z.object({
	email: z.string().email().optional(),
	name: z.string().trim().min(1).max(100),
	disabled: z.boolean().optional(),
	password: z.preprocess(
		(value) => (typeof value === "string" ? value.trim() : value),
		z.string().min(8).or(z.literal("")).optional().transform((value) => value || null),
	),
});

export const mailboxAccessSchema = z.object({
	userId: z.string().min(1),
	permission: z.enum(["read_only", "send_as", "send_on_behalf", "full_access"]),
});

export const accountMailboxAccessSchema = z.object({
	mailboxId: z.string().min(1),
	permission: z.enum(["read_only", "send_as", "send_on_behalf", "full_access"]),
});

export const accountMailboxSchema = z.object({
	domainId: z.string().min(1),
	localPart: z.string().min(1).max(64).regex(/^[a-zA-Z0-9._%+-]+$/),
	displayName: z.string().trim().max(100).optional(),
});

export const updateMailboxSchema = z.object({
	displayName: z.string().max(100).nullable().optional(),
});

export const folderSchema = z.object({
	mailboxId: z.string().min(1),
	name: z.string().trim().min(1).max(80),
});

export const updateProfileSchema = z.object({
	name: z.string().trim().min(1).max(100),
	resetEmail: z.preprocess(
		(value) => (typeof value === "string" ? value.trim() : value),
		z.string().email().or(z.literal("")).transform((value) => value || null),
	),
});

export const routingRuleSchema = z.object({
	domainId: z.string().optional(),
	pattern: z.string().trim().min(1).max(200).optional(),
	matchField: z.enum(["email", "content", "title"]).default("email"),
	matchOperator: z.enum(["contains", "exact"]).default("contains"),
	matchValue: z.string().trim().min(1).max(500),
	action: z.enum(["store", "forward", "reject", "spam", "trash"]).optional(),
	mailboxId: z.string().min(1),
	folderId: z.string().optional(),
	destination: z.string().min(1).optional(),
	forwardTo: z.string().email().optional(),
	priority: z.number().int().default(0),
});

export const webhookSchema = z.object({
	url: z.string().url().max(2048),
	events: z
		.array(z.enum(["message.inbound", "message.outbound", "message.failed"]))
		.min(1)
		.max(3),
	/** Scope the webhook to one of the caller's mailboxes. */
	mailboxId: z.string().min(1).max(128).optional(),
});
