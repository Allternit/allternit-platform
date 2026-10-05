import { NextResponse } from "next/server";
import { and, eq, isNull } from "drizzle-orm";
import { getEnv } from "@/lib/cloudflare";
import { getDb } from "@/db";
import { apiKeys } from "@/db/schema";
import { authenticateSessionOrApiKey } from "@/lib/api/auth";
import { createAuditLog } from "@/lib/mailboxes/audit";

type ApiKeyRouteParams = {
	params: Promise<{ id: string }>;
};

export async function DELETE(request: Request, { params }: ApiKeyRouteParams) {
	const env = getEnv();
	// A signed-in user, or an admin-scope API key (the platform tears down bot mail).
	const user = (await authenticateSessionOrApiKey(env, request, { scope: "admin" }))?.user;
	if (!user) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	const { id } = await params;

	const db = getDb(env);
	// Soft revoke: keep the row (and its prefix/hash history) but refuse further auth.
	const revoked = await db
		.update(apiKeys)
		.set({ revokedAt: new Date() })
		.where(and(eq(apiKeys.id, id), eq(apiKeys.userId, user.id), isNull(apiKeys.revokedAt)))
		.returning({ id: apiKeys.id });

	if (revoked.length === 0) {
		return NextResponse.json({ error: "API key not found" }, { status: 404 });
	}

	await createAuditLog(env, {
		actorUserId: user.id,
		action: "api_key.revoke",
		metadata: { apiKeyId: id },
	});

	return NextResponse.json({ id, revoked: true });
}
