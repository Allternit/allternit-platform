import { NextResponse } from "next/server";
import { getEnv } from "@/lib/cloudflare";
import { apiKeyAllowsMailbox, authenticateApiKey, requireScope } from "@/lib/api/auth";
import { sendEmailSchema } from "@/lib/validators";
import { sendEmail } from "@/lib/email/send";
import { compactOutboundHeaders } from "@/lib/email/custom-headers";
import { decodeBase64Content } from "@/lib/email/attachments";
import { readJsonBody } from "@/lib/http/request";
import { RequestBodyTooLargeError } from "@/lib/http/errors";
import { getSendErrorStatus } from "@/app/api/send/error-utils";

const MAX_IDEMPOTENCY_KEY_LENGTH = 200;

export async function POST(request: Request) {
	const env = getEnv();
	const auth = await authenticateApiKey(env, request.headers.get("authorization"));
	if (!auth || !requireScope(auth.scopes, "send")) {
		return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	}

	// Per-key send rate limit (SEND_RATE_LIMIT binding); keyed by key prefix.
	if (env.SEND_RATE_LIMIT) {
		try {
			const outcome = await env.SEND_RATE_LIMIT.limit({ key: auth.prefix });
			if (!outcome.success) {
				return NextResponse.json({ error: "Rate limit exceeded" }, { status: 429 });
			}
		} catch (error) {
			console.warn("Send rate limiter unavailable", error);
		}
	}

	let body: unknown;
	try {
		body = await readJsonBody(request, 30 * 1024 * 1024);
	} catch (error) {
		const status = error instanceof RequestBodyTooLargeError ? 413 : 400;
		return NextResponse.json({ error: "Invalid send request" }, { status });
	}
	const parsed = sendEmailSchema.safeParse(body);
	if (!parsed.success) {
		return NextResponse.json({ error: parsed.error.flatten() }, { status: 400 });
	}

	if (!apiKeyAllowsMailbox(auth, parsed.data.mailboxId)) {
		return NextResponse.json(
			{ error: "API key is not authorized for this mailbox" },
			{ status: 403 },
		);
	}

	const idempotencyKeyHeader = request.headers.get("idempotency-key");
	const idempotencyKey =
		idempotencyKeyHeader && idempotencyKeyHeader.length <= MAX_IDEMPOTENCY_KEY_LENGTH
			? idempotencyKeyHeader
			: null;

	try {
		const { attachments, skipApproval, headers, ...fields } = parsed.data;
		// Only an admin-scope key may bypass the approval gate. A mailbox-scoped
		// key saying skipApproval is ignored — it must never send unreviewed.
		const adminSkipApproval = skipApproval === true && requireScope(auth.scopes, "admin");
		const result = await sendEmail(
			env,
			{
				userId: auth.userId,
				...fields,
				headers: compactOutboundHeaders(headers),
				attachments: attachments?.map((attachment) => ({
					filename: attachment.filename,
					type: attachment.type,
					content: decodeBase64Content(attachment.contentBase64),
					disposition: "attachment",
				})),
			},
			{ idempotencyKey, skipApproval: adminSkipApproval },
		);
		return NextResponse.json(result);
	} catch (err) {
		const message = err instanceof Error ? err.message : "Send failed";
		return NextResponse.json({ error: message }, { status: getSendErrorStatus(message) });
	}
}
