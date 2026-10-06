/**
 * Standard Webhooks (https://www.standardwebhooks.com) signing and
 * verification: the format MCP Events requires for webhook delivery.
 * Mirrors `mcp/protocol/src/webhooks.rs`.
 *
 *   webhook-id:        <message id, unique per event, stable across retries>
 *   webhook-timestamp: <unix seconds>
 *   webhook-signature: v1,<base64 HMAC-SHA256(key, "<id>.<timestamp>.<body>")>
 *
 * Secrets are `whsec_` + base64 of 24-64 random bytes. For MCP Events the
 * client supplies the secret on `events/subscribe`; the server never mints it.
 */
import { createHmac, timingSafeEqual } from "node:crypto";

export const SECRET_PREFIX = "whsec_";
export const MIN_KEY_BYTES = 24;
export const MAX_KEY_BYTES = 64;
/** Receivers reject timestamps further than this from now. */
export const TOLERANCE_SECS = 5 * 60;

export const HEADER_ID = "webhook-id";
export const HEADER_TIMESTAMP = "webhook-timestamp";
export const HEADER_SIGNATURE = "webhook-signature";

export type SecretErrorKind = "missing_prefix" | "not_base64" | "bad_length";

export class SecretError extends Error {
  constructor(readonly kind: SecretErrorKind, message: string) {
    super(message);
    this.name = "SecretError";
  }
}

const BASE64_RE = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

/** Decode a `whsec_...` secret to its key bytes, enforcing the 24-64 byte range MCP Events requires. */
export function parseSecret(secret: string): Uint8Array {
  if (!secret.startsWith(SECRET_PREFIX)) throw new SecretError("missing_prefix", `secret must start with ${SECRET_PREFIX}`);
  const b64 = secret.slice(SECRET_PREFIX.length);
  if (!b64 || !BASE64_RE.test(b64)) throw new SecretError("not_base64", `secret is not valid base64 after ${SECRET_PREFIX}`);
  const key = Buffer.from(b64, "base64");
  if (key.length < MIN_KEY_BYTES || key.length > MAX_KEY_BYTES) {
    throw new SecretError("bad_length", `secret decodes to ${key.length} bytes; must be ${MIN_KEY_BYTES}-${MAX_KEY_BYTES}`);
  }
  return new Uint8Array(key);
}

const bytes = (body: string | Uint8Array) => (typeof body === "string" ? Buffer.from(body, "utf8") : body);

/** `v1,<base64 signature>` for one message. */
export function sign(key: Uint8Array, msgId: string, timestamp: number, body: string | Uint8Array): string {
  const mac = createHmac("sha256", key);
  mac.update(`${msgId}.${Math.trunc(timestamp)}.`);
  mac.update(bytes(body));
  return `v1,${mac.digest("base64")}`;
}

/**
 * The three headers for one delivery. Callers add `Content-Type` and any
 * transport-specific headers (MCP Events adds `X-MCP-Subscription-Id`).
 */
export function signHeaders(key: Uint8Array, msgId: string, timestamp: number, body: string | Uint8Array): Record<string, string> {
  return {
    [HEADER_ID]: msgId,
    [HEADER_TIMESTAMP]: String(Math.trunc(timestamp)),
    [HEADER_SIGNATURE]: sign(key, msgId, timestamp, body),
  };
}

export type VerifyErrorKind = "bad_timestamp" | "expired" | "no_matching_signature";

export class VerifyError extends Error {
  constructor(readonly kind: VerifyErrorKind) {
    super(`webhook verification failed: ${kind}`);
    this.name = "VerifyError";
  }
}

/**
 * Verify a received message; throws {@link VerifyError} on failure.
 * `signatureHeader` may hold several space-separated signatures (key
 * rotation); any `v1` match passes. `nowUnix` defaults to the current time.
 */
export function verify(
  key: Uint8Array,
  msgId: string,
  timestampHeader: string,
  signatureHeader: string,
  body: string | Uint8Array,
  nowUnix: number = Math.floor(Date.now() / 1000),
): void {
  const raw = timestampHeader.trim();
  if (!/^-?\d+$/.test(raw)) throw new VerifyError("bad_timestamp");
  const ts = Number(raw);
  if (!Number.isSafeInteger(ts)) throw new VerifyError("bad_timestamp");
  if (Math.abs(nowUnix - ts) > TOLERANCE_SECS) throw new VerifyError("expired");
  const expected = Buffer.from(sign(key, msgId, ts, body));
  const ok = signatureHeader
    .split(" ")
    .filter((s) => s.startsWith("v1,"))
    .some((s) => {
      const got = Buffer.from(s);
      return got.length === expected.length && timingSafeEqual(got, expected);
    });
  if (!ok) throw new VerifyError("no_matching_signature");
}

/** Read the three Standard Webhooks headers off a Fetch `Headers` (or plain record) and verify. */
export function verifyRequest(
  key: Uint8Array,
  headers: Headers | Record<string, string | undefined>,
  body: string | Uint8Array,
  nowUnix?: number,
): { msgId: string; timestamp: number } {
  const get = (name: string) =>
    headers instanceof Headers ? headers.get(name) ?? "" : (headers[name] ?? headers[name.toLowerCase()] ?? "");
  const msgId = get(HEADER_ID);
  const ts = get(HEADER_TIMESTAMP);
  verify(key, msgId, ts, get(HEADER_SIGNATURE), body, nowUnix);
  return { msgId, timestamp: Number(ts) };
}
