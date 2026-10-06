import { NextResponse } from "next/server";
import { getEnv } from "@/lib/cloudflare";
import { authenticateSessionOrApiKey } from "@/lib/api/auth";
import { relayConfigured } from "@/lib/email/relay";
import { addRelayDomain, checkRelayDomain, RelayDomainError, removeRelayDomain } from "@/lib/domains/relay-domains";

type Ctx = { params: Promise<{ host: string }> };

async function relayDomainRoute(request: Request, ctx: Ctx, run: (env: CloudflareEnv, userId: string, host: string) => Promise<unknown>) {
	const env = getEnv();
	const auth = await authenticateSessionOrApiKey(env, request, { scope: "admin" });
	if (!auth) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	if (!relayConfigured(env)) return NextResponse.json({ error: "custom_domains_unavailable" }, { status: 503 });
	const host = decodeURIComponent((await ctx.params).host).trim().toLowerCase().replace(/\.$/, "");
	try {
		return NextResponse.json((await run(env, auth.user.id, host)) ?? { ok: true });
	} catch (err) {
		if (err instanceof RelayDomainError) return NextResponse.json({ error: err.code }, { status: err.status });
		console.error("relay domain route failed", err);
		return NextResponse.json({ error: "relay_unavailable" }, { status: 502 });
	}
}

/** Add a customer domain: returns the DNS records to add. Safe to repeat. */
export async function PUT(request: Request, ctx: Ctx) {
	return relayDomainRoute(request, ctx, addRelayDomain);
}

/** Check the records live (activates the domain when they all pass). */
export async function GET(request: Request, ctx: Ctx) {
	return relayDomainRoute(request, ctx, checkRelayDomain);
}

/** Remove a domain that no longer has mailboxes. */
export async function DELETE(request: Request, ctx: Ctx) {
	return relayDomainRoute(request, ctx, removeRelayDomain);
}
