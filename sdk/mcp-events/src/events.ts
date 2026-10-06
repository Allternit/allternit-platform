/**
 * MCP Events extension wire types and validation (draft:
 * modelcontextprotocol/experimental-ext-triggers-events; the webhook profile
 * ChatGPT implements: developers.openai.com/plugins/build/mcp-events).
 * Mirrors `mcp/protocol/src/events.rs`. Storage, authorization and delivery
 * live with the server that owns the subscriptions.
 */
import { createHash } from "node:crypto";
import { isIP } from "node:net";
import { parseSecret } from "./webhooks.js";

/** `events/*` error codes (draft extension; implementation-defined range). */
export const EventsErrorCode = {
  NotFound: -32011,
  Forbidden: -32012,
  ResourceExhausted: -32013,
  Unsupported: -32014,
  CallbackEndpointError: -32015,
} as const;
export type EventsErrorCode = (typeof EventsErrorCode)[keyof typeof EventsErrorCode];

/** Largest event body a receiver must accept (ChatGPT: 256 KiB). */
export const MAX_EVENT_BYTES = 262_144;
/** Header naming the subscription on every webhook delivery. */
export const HEADER_SUBSCRIPTION_ID = "X-MCP-Subscription-Id";

export const EVENTS_METHODS = {
  list: "events/list",
  subscribe: "events/subscribe",
  unsubscribe: "events/unsubscribe",
} as const;

export type DeliveryMode = "webhook" | "poll" | "push";

/** Capability a server advertises (in `server/discover` / `initialize`) when it serves `events/*`. */
export interface EventsCapability {
  listChanged?: boolean;
}

type JsonObject = Record<string, unknown>;

/** One entry in `events/list`. */
export interface EventDef {
  name: string;
  description: string;
  delivery: DeliveryMode[];
  inputSchema: JsonObject;
  payloadSchema: JsonObject;
}

export interface EventsListParams {
  cursor?: string;
}
export interface EventsListResult {
  events: EventDef[];
  nextCursor?: string;
}

export interface Delivery {
  mode: DeliveryMode;
  url?: string;
  secret?: string;
}

/** `events/subscribe` params. `ttlMs` absent = server default; `null` = asks for no expiry. */
export interface SubscribeParams {
  name: string;
  arguments?: JsonObject;
  delivery: Delivery;
  cursor?: string | null;
  ttlMs?: number | null;
}

export interface DeliveryStatus {
  active?: boolean;
  lastDeliveryAt?: string | null;
  lastError?: string | null;
  failedSince?: string | null;
  throttled?: boolean;
  retryAfterMs?: number | null;
}

export interface SubscribeResult {
  /** Server-derived subscription handle (see {@link subscriptionId}). */
  id: string;
  /** ISO 8601 expiry the client must refresh before, or null for no expiry. */
  refreshBefore: string | null;
  /** Safe-to-persist watermark. */
  cursor: string | null;
  /** True when events between the requested cursor and now were skipped. */
  truncated: boolean;
  deliveryStatus?: DeliveryStatus;
}

/** `events/unsubscribe` params. */
export interface UnsubscribeParams {
  name: string;
  arguments?: JsonObject;
  delivery: Delivery;
}
export type UnsubscribeResult = Record<string, never>;

/** Body of one event delivery. */
export interface EventEnvelope<T = unknown> {
  eventId: string;
  name: string;
  timestamp: string;
  data: T;
  cursor: string | null;
}
export interface VerificationEnvelope {
  type: "verification";
  challenge: string;
}
export interface GapEnvelope {
  type: "gap";
  cursor: string;
}
export interface TerminatedEnvelope {
  type: "terminated";
  subscriptionId: string;
  error: { code: number; message: string; data?: unknown };
}
export type ControlEnvelope = VerificationEnvelope | GapEnvelope | TerminatedEnvelope;

export function eventEnvelope<T>(eventId: string, name: string, timestamp: string | Date, data: T, cursor?: string | null): EventEnvelope<T> {
  return { eventId, name, timestamp: typeof timestamp === "string" ? timestamp : timestamp.toISOString(), data, cursor: cursor ?? null };
}

/** Control envelope sent before activating an unverified callback; the receiver must echo `{ challenge }`. */
export function verificationEnvelope(challenge: string): VerificationEnvelope {
  return { type: "verification", challenge };
}

/** Control envelope telling the receiver events were skipped; resume from `cursor`. */
export function gapEnvelope(cursor: string): GapEnvelope {
  return { type: "gap", cursor };
}

/** Control envelope ending a subscription (auth revoked, event removed). */
export function terminatedEnvelope(subscriptionId: string, code: number, message: string): TerminatedEnvelope {
  return { type: "terminated", subscriptionId, error: { code, message } };
}

/**
 * Validate the webhook half of a subscribe/unsubscribe: an https URL that is
 * not an obviously private target, and (for subscribe) a `whsec_` secret of
 * 24-64 bytes. DNS resolution must be re-checked at delivery time by the
 * sender; a literal check here is not enough on its own. Throws an `Error`
 * with a client-safe message.
 */
export function validateWebhook(delivery: Delivery, needSecret: boolean): { url: string; key?: Uint8Array } {
  if (delivery?.mode !== "webhook") throw new Error("only webhook delivery is offered");
  const url = delivery.url;
  if (!url) throw new Error("delivery.url is required");
  validateCallbackUrl(url);
  if (delivery.secret !== undefined && delivery.secret !== null) return { url, key: parseSecret(delivery.secret) };
  if (needSecret) throw new Error("delivery.secret is required");
  return { url };
}

/** `https://` only, no credentials in the URL, no localhost / private / link-local / loopback IP literals. Throws on failure. */
export function validateCallbackUrl(url: string): void {
  if (!url.startsWith("https://")) throw new Error("callback URL must use https://");
  const rest = url.slice("https://".length);
  const authority = rest.split(/[/?#]/)[0] ?? "";
  if (!authority) throw new Error("callback URL has no host");
  if (authority.includes("@")) throw new Error("callback URL must not carry credentials");
  let host: string;
  if (authority.startsWith("[")) host = authority.slice(1).split("]")[0] ?? "";
  else {
    const i = authority.lastIndexOf(":");
    host = i >= 0 ? authority.slice(0, i) : authority;
  }
  const lower = host.toLowerCase();
  if (lower === "localhost" || lower.endsWith(".localhost") || lower.endsWith(".local") || lower.endsWith(".internal")) {
    throw new Error("callback URL points at a private host");
  }
  if (isIP(host) && !isPublicIp(host)) throw new Error("callback URL points at a private address");
}

function v4Octets(ip: string): number[] {
  return ip.split(".").map(Number);
}

function isPublicV4(o: number[]): boolean {
  const [a, b, c, d] = o as [number, number, number, number];
  return !(
    a === 127 || // loopback
    a === 10 ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) || // private
    (a === 169 && b === 254) || // link-local
    (a === 255 && b === 255 && c === 255 && d === 255) || // broadcast
    a === 0 || // unspecified + "this network"
    (a >= 224 && a <= 239) || // multicast
    (a === 192 && b === 0 && c === 2) ||
    (a === 198 && b === 51 && c === 100) ||
    (a === 203 && b === 0 && c === 113) || // documentation
    (a === 100 && b >= 64 && b <= 127) || // CGNAT
    (a === 192 && b === 0 && c === 0) ||
    (a === 198 && (b === 18 || b === 19)) || // benchmarking
    a >= 240
  );
}

/** Expand an IPv6 literal to 8 16-bit segments (handles `::` and a trailing dotted IPv4). */
function v6Segments(ip: string): number[] {
  let s = ip.split("%")[0]!;
  const lastColon = s.lastIndexOf(":");
  const last = s.slice(lastColon + 1);
  if (last.includes(".")) {
    const o = v4Octets(last);
    s = `${s.slice(0, lastColon + 1)}${((o[0]! << 8) | o[1]!).toString(16)}:${((o[2]! << 8) | o[3]!).toString(16)}`;
  }
  const parse = (p: string) => (p ? p.split(":").map((x) => parseInt(x, 16)) : []);
  if (!s.includes("::")) return parse(s);
  const [head, tail] = s.split("::") as [string, string];
  const h = parse(head);
  const t = parse(tail);
  return [...h, ...new Array(8 - h.length - t.length).fill(0), ...t];
}

/** False for loopback, private, link-local, CGNAT, multicast, unspecified, documentation and unique-local ranges. */
export function isPublicIp(ip: string): boolean {
  const kind = isIP(ip);
  if (kind === 4) return isPublicV4(v4Octets(ip));
  if (kind !== 6) return false;
  const s = v6Segments(ip);
  // IPv4-mapped ::ffff:a.b.c.d
  if (s.slice(0, 5).every((x) => x === 0) && s[5] === 0xffff) {
    return isPublicV4([s[6]! >> 8, s[6]! & 0xff, s[7]! >> 8, s[7]! & 0xff]);
  }
  const allZero = s.every((x) => x === 0);
  const loopback = s.slice(0, 7).every((x) => x === 0) && s[7] === 1;
  return !(
    loopback ||
    allZero ||
    (s[0]! & 0xff00) === 0xff00 || // multicast
    (s[0]! & 0xfe00) === 0xfc00 || // unique local
    (s[0]! & 0xffc0) === 0xfe80 || // link-local
    (s[0] === 0x2001 && s[1] === 0x0db8) // documentation
  );
}

/** JSON with object keys sorted, so `{"a":1,"b":2}` and `{"b":2,"a":1}` identify the same subscription. */
export function canonicalJson(v: unknown): string {
  const sort = (x: unknown): unknown => {
    if (Array.isArray(x)) return x.map(sort);
    if (x && typeof x === "object") {
      const out: Record<string, unknown> = {};
      for (const k of Object.keys(x as object).sort()) out[k] = sort((x as Record<string, unknown>)[k]);
      return out;
    }
    return x;
  };
  return JSON.stringify(sort(v ?? {}));
}

/**
 * Deterministic subscription id for the identity key
 * `(principal, url, name, arguments)`: the same key always maps to the same
 * id, which makes `events/subscribe` idempotent. Byte-identical to the Rust
 * crate for integer/string/bool/null arguments.
 */
export function subscriptionId(principal: string, url: string, name: string, args: unknown): string {
  const h = createHash("sha256");
  for (const part of [principal, url, name, canonicalJson(args)]) {
    const b = Buffer.from(part, "utf8");
    const len = Buffer.alloc(8);
    len.writeBigUInt64BE(BigInt(b.length));
    h.update(len);
    h.update(b);
  }
  return `sub_${h.digest().subarray(0, 12).toString("hex")}`;
}
