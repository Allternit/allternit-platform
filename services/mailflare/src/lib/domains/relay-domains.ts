/**
 * Customer domains (transport `relay`): added on the relay, which makes the
 * domain's DKIM key and lists the DNS records; active once every record checks.
 */

import { and, eq } from "drizzle-orm";
import { getDb } from "@/db";
import { domains, mailboxes } from "@/db/schema";
import { newId } from "@/lib/ids";
import { relayFetch } from "@/lib/email/relay";

export type RelayRecord = { type: string; host: string; value: string; purpose: string; ok: boolean | null };
export type RelayDomainView = { id: string; domain: string; status: "pending" | "active" | "error"; verified: boolean; records: RelayRecord[] };

export class RelayDomainError extends Error {
	constructor(readonly status: number, readonly code: string) {
		super(code);
	}
}

async function relayJson(env: CloudflareEnv, path: string, init: RequestInit = {}) {
	const res = await relayFetch(env, path, init);
	const body = (await res.json().catch(() => ({}))) as Record<string, unknown>;
	if (!res.ok) throw new RelayDomainError(res.status === 400 || res.status === 404 ? res.status : 502, String(body.error ?? `relay_${res.status}`));
	return body as { domain: string; verified: boolean; records: RelayRecord[] };
}

async function ownedRow(env: CloudflareEnv, userId: string, host: string) {
	const [row] = await getDb(env).select().from(domains).where(eq(domains.hostname, host)).limit(1);
	if (row && (row.userId !== userId || row.transport !== "relay")) throw new RelayDomainError(409, "domain_taken");
	return row ?? null;
}

const view = (id: string, status: RelayDomainView["status"], r: { domain: string; verified: boolean; records: RelayRecord[] }): RelayDomainView => ({
	id,
	domain: r.domain,
	status,
	verified: r.verified,
	records: r.records,
});

export async function addRelayDomain(env: CloudflareEnv, userId: string, host: string): Promise<RelayDomainView> {
	const existing = await ownedRow(env, userId, host);
	const relay = await relayJson(env, `/domains/${encodeURIComponent(host)}`, { method: "PUT" });
	const id = existing?.id ?? newId("dom");
	if (!existing) {
		await getDb(env).insert(domains).values({
			id,
			userId,
			hostname: relay.domain,
			zoneId: "relay",
			transport: "relay",
			status: "pending",
			routingEnabled: false,
			sendingEnabled: false,
		});
	}
	return view(id, existing?.status ?? "pending", relay);
}

/** Checks the DNS records live; a domain whose records all check becomes active. */
export async function checkRelayDomain(env: CloudflareEnv, userId: string, host: string): Promise<RelayDomainView> {
	const row = await ownedRow(env, userId, host);
	if (!row) throw new RelayDomainError(404, "domain_not_found");
	const relay = await relayJson(env, `/domains/${encodeURIComponent(host)}/check`, { method: "POST" });
	// A domain stays active once verified: a record briefly failing to resolve
	// must not cut off live mail. The relay itself refuses unverified domains.
	const status = relay.verified ? "active" : row.status === "active" ? "active" : "pending";
	await getDb(env)
		.update(domains)
		.set({ status, routingEnabled: status === "active", sendingEnabled: status === "active" })
		.where(eq(domains.id, row.id));
	return view(row.id, status, relay);
}

export async function removeRelayDomain(env: CloudflareEnv, userId: string, host: string): Promise<void> {
	const row = await ownedRow(env, userId, host);
	if (!row) throw new RelayDomainError(404, "domain_not_found");
	const [box] = await getDb(env).select({ id: mailboxes.id }).from(mailboxes).where(and(eq(mailboxes.domainId, row.id))).limit(1);
	if (box) throw new RelayDomainError(409, "domain_has_mailboxes");
	await relayJson(env, `/domains/${encodeURIComponent(host)}`, { method: "DELETE" }).catch((err) => {
		if (!(err instanceof RelayDomainError && err.status === 404)) throw err;
	});
	await getDb(env).delete(domains).where(eq(domains.id, row.id));
}
