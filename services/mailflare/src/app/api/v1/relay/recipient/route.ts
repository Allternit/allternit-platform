import { NextResponse } from "next/server";
import { getEnv } from "@/lib/cloudflare";
import { getDb } from "@/db";
import { relayAuthorized } from "@/lib/email/relay";
import { resolveInboundAddress } from "@/lib/email/routing";

/** The relay asks before Postfix accepts mail: does this address have a mailbox? */
export async function GET(request: Request) {
	const env = getEnv();
	if (!relayAuthorized(env, request)) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	const address = (new URL(request.url).searchParams.get("address") ?? "").trim().toLowerCase();
	if (!address) return NextResponse.json({ exists: false });
	const decision = await resolveInboundAddress(getDb(env), address);
	return NextResponse.json({ exists: decision?.action === "store" });
}
