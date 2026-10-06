/**
 * Customer domains on Allternit's own mail host (services/mail-relay on
 * mx.allternit.com). The relay calls this worker with MAIL_RELAY_SECRET for
 * inbound mail and recipient checks; this worker calls the relay with the same
 * secret to add domains, check their DNS and send.
 */

import { and, eq } from "drizzle-orm";
import { getDb } from "@/db";
import { domains } from "@/db/schema";

export function relayConfigured(env: CloudflareEnv): boolean {
	return Boolean(env.MAIL_RELAY_URL && env.MAIL_RELAY_SECRET && env.MAIL_RELAY_SECRET.length >= 32);
}

/** Constant-time check of `Authorization: Bearer <MAIL_RELAY_SECRET>`. */
export function relayAuthorized(env: CloudflareEnv, request: Request): boolean {
	if (!relayConfigured(env)) return false;
	const got = new TextEncoder().encode((request.headers.get("authorization") ?? "").replace(/^Bearer\s+/i, ""));
	const want = new TextEncoder().encode(env.MAIL_RELAY_SECRET!);
	if (got.length !== want.length) return false;
	let diff = 0;
	for (let i = 0; i < got.length; i++) diff |= got[i] ^ want[i];
	return diff === 0;
}

export async function relayFetch(env: CloudflareEnv, path: string, init: RequestInit = {}): Promise<Response> {
	if (!relayConfigured(env)) throw new Error("Customer domains are not configured on this deployment");
	const base = env.MAIL_RELAY_URL!.replace(/\/$/, "");
	return fetch(`${base}${path}`, {
		...init,
		headers: { ...(init.headers as Record<string, string> | undefined), authorization: `Bearer ${env.MAIL_RELAY_SECRET}`, "content-type": "application/json" },
	});
}

/** Domain part of a bare address or a "Name <addr>" form, lower-cased. */
export function addressDomain(address: string): string | null {
	const addr = (address.match(/<([^<>]+)>\s*$/)?.[1] ?? address).trim();
	const at = addr.lastIndexOf("@");
	return at > 0 ? addr.slice(at + 1).toLowerCase().replace(/\.$/, "") : null;
}

/** Whether mail from this address goes out through the relay (an active relay domain). */
export async function isRelaySender(env: CloudflareEnv, from: string): Promise<boolean> {
	const host = addressDomain(from);
	if (!host) return false;
	const [row] = await getDb(env)
		.select({ id: domains.id })
		.from(domains)
		.where(and(eq(domains.hostname, host), eq(domains.transport, "relay"), eq(domains.status, "active")))
		.limit(1);
	return Boolean(row);
}
