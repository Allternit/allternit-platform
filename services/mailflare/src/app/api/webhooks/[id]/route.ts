import { NextResponse } from "next/server";
import { and, eq } from "drizzle-orm";
import { getEnv } from "@/lib/cloudflare";
import { getDb } from "@/db";
import { webhooks } from "@/db/schema";
import { authenticateSessionOrApiKey } from "@/lib/api/auth";
import { createAuditLog } from "@/lib/mailboxes/audit";

export type WebhookRouteParams = {
	params: Promise<{ id: string }>;
};

export async function DELETE(request: Request, { params }: WebhookRouteParams) {
	const env = getEnv();
	// A signed-in user, or an admin-scope API key (the platform tears down bot mail).
	const user = (await authenticateSessionOrApiKey(env, request, { scope: "admin" }))?.user;
	if (!user) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	const { id } = await params;

	const db = getDb(env);
	const deleted = await db
		.delete(webhooks)
		.where(and(eq(webhooks.id, id), eq(webhooks.userId, user.id)))
		.returning({ id: webhooks.id });

	if (deleted.length === 0) {
		return NextResponse.json({ error: "Webhook not found" }, { status: 404 });
	}

	await createAuditLog(env, {
		actorUserId: user.id,
		action: "webhook.delete",
		metadata: { webhookId: id },
	});

	return NextResponse.json({ id, deleted: true });
}
