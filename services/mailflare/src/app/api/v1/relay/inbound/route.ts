import { NextResponse } from "next/server";
import { getEnv } from "@/lib/cloudflare";
import { getDb } from "@/db";
import { relayAuthorized } from "@/lib/email/relay";
import { resolveInboundAddress } from "@/lib/email/routing";
import { storeRawToR2 } from "@/lib/email/inbound";

const MAX_BYTES = 25 * 1024 * 1024;

/**
 * Mail for a customer domain, handed in by the relay on mx.allternit.com after
 * Postfix accepted it. Same path as Cloudflare Email Routing mail from here on:
 * raw MIME to R2, then the inbound queue. A recipient with no mailbox is
 * acknowledged and dropped (Postfix already checked it; a retry can't help).
 */
export async function POST(request: Request) {
	const env = getEnv();
	if (!relayAuthorized(env, request)) return NextResponse.json({ error: "Unauthorized" }, { status: 401 });
	const from = request.headers.get("x-relay-from") ?? "";
	const to = (request.headers.get("x-relay-to") ?? "").trim().toLowerCase();
	if (!to || !request.body) return NextResponse.json({ error: "missing_fields" }, { status: 400 });
	if (Number(request.headers.get("content-length") ?? 0) > MAX_BYTES) return NextResponse.json({ error: "too_large" }, { status: 413 });

	const decision = await resolveInboundAddress(getDb(env), to);
	if (!decision || decision.action !== "store") return NextResponse.json({ ok: true, dropped: true }, { status: 202 });

	const rawR2Key = await storeRawToR2(env, from, to, request.body);
	await env.INBOUND_QUEUE.send({ from, to, rawR2Key, headers: {} });
	return NextResponse.json({ ok: true }, { status: 202 });
}
