import { NextResponse } from "next/server";
import { and, eq } from "drizzle-orm";
import { getEnv } from "@/lib/cloudflare";
import { getDb } from "@/db";
import { mailboxes, webhooks } from "@/db/schema";
import { authenticateSessionOrApiKey } from "@/lib/api/auth";
import { newId } from "@/lib/ids";
import { webhookSchema } from "@/lib/validators";
import { readJsonBody } from "@/lib/http/request";
import { RequestBodyTooLargeError } from "@/lib/http/errors";

/** A signed-in user, or an admin-scope API key (the platform provisions bot webhooks). */
async function caller(env: CloudflareEnv, request: Request) {
	const auth = await authenticateSessionOrApiKey(env, request, { scope: "admin" });
	return auth?.user ?? null;
}

export async function GET(request: Request) {
	const env = getEnv();
	const user = await caller(env, request);
	if (!user) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	const db = getDb(env);
	const rows = await db.select().from(webhooks).where(eq(webhooks.userId, user.id));
	return NextResponse.json({
		webhooks: rows.map((w) => ({ id: w.id, url: w.url, events: w.events, enabled: w.enabled, mailboxId: w.mailboxId ?? null })),
	});
}

export async function POST(request: Request) {
	const env = getEnv();
	const user = await caller(env, request);
	if (!user) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	let body: unknown;
	try {
		body = await readJsonBody(request, 16 * 1024);
	} catch (error) {
		const status = error instanceof RequestBodyTooLargeError ? 413 : 400;
		return NextResponse.json({ error: "Invalid webhook request" }, { status });
	}
	const parsed = webhookSchema.safeParse(body);
	if (!parsed.success) {
		return NextResponse.json({ error: parsed.error.flatten() }, { status: 400 });
	}

	const db = getDb(env);
	const mailboxId = parsed.data.mailboxId ?? null;
	if (mailboxId) {
		const owned = await db.select({ id: mailboxes.id }).from(mailboxes).where(and(eq(mailboxes.id, mailboxId), eq(mailboxes.userId, user.id))).limit(1);
		if (owned.length === 0) return NextResponse.json({ error: "Mailbox not found" }, { status: 404 });
	}
	const secret = newId("whsec");
	const id = newId("wh");
	await db.insert(webhooks).values({
		id,
		userId: user.id,
		url: parsed.data.url,
		secret,
		events: JSON.stringify(parsed.data.events),
		enabled: true,
		mailboxId,
	});

	return NextResponse.json({ id, url: parsed.data.url, secret, events: parsed.data.events, mailboxId });
}
