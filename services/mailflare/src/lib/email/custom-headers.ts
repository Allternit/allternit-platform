/**
 * Custom outbound headers for the agent-email reply path.
 *
 * The send API accepts an optional `headers` object so replies can thread
 * (In-Reply-To / References) and mark themselves as automatic. Only the
 * headers below may be set by a caller — anything else is rejected — and a
 * mailbox-scoped key can never smuggle a skip of the approval gate (that is a
 * separate, admin-only flag handled in the route).
 *
 * This module is dependency-free on purpose: it is unit-tested with the
 * built-in node test runner (`node --test`), no install step required.
 */

export const ALLOWED_OUTBOUND_HEADERS = [
	"In-Reply-To",
	"References",
	"Auto-Submitted",
	"List-Unsubscribe",
] as const;

export type OutboundHeaders = Record<string, string>;

export type NormalizeOutboundHeadersResult =
	| { ok: true; headers: OutboundHeaders }
	| { ok: false; rejected: string[] };

/**
 * Validate and normalize a caller-supplied `headers` value. Keys are matched
 * case-insensitively and returned in canonical case; duplicate keys in a
 * different case are rejected rather than merged. Non-object input is an
 * error (the empty object is fine).
 */
export function normalizeOutboundHeaders(input: unknown): NormalizeOutboundHeadersResult {
	if (input === undefined || input === null) return { ok: true, headers: {} };
	if (typeof input !== "object" || Array.isArray(input)) {
		return { ok: false, rejected: ["headers must be an object"] };
	}
	const canonical = new Map(ALLOWED_OUTBOUND_HEADERS.map((h) => [h.toLowerCase(), h]));
	const seen = new Set<string>();
	const headers: OutboundHeaders = {};
	const rejected: string[] = [];
	for (const [key, value] of Object.entries(input as Record<string, unknown>)) {
		const allowed = canonical.get(key.toLowerCase());
		if (!allowed) {
			rejected.push(key);
			continue;
		}
		if (seen.has(allowed)) {
			rejected.push(key);
			continue;
		}
		if (typeof value !== "string" || value.length === 0) {
			rejected.push(key);
			continue;
		}
		seen.add(allowed);
		headers[allowed] = value.length > 1000 ? value.slice(0, 1000) : value;
	}
	return rejected.length > 0 ? { ok: false, rejected } : { ok: true, headers };
}

/**
 * Zod's output type for the optional per-key header schema is
 * `Record<string, string | undefined>`; delivery wants defined values only.
 */
export function compactOutboundHeaders(
	input: Record<string, string | undefined> | undefined,
): OutboundHeaders | undefined {
	if (!input) return undefined;
	const out: OutboundHeaders = {};
	for (const [key, value] of Object.entries(input)) {
		if (value !== undefined) out[key] = value;
	}
	return Object.keys(out).length > 0 ? out : undefined;
}
