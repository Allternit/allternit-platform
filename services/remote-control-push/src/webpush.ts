/**
 * Web Push message encryption (RFC 8291, `aes128gcm` content coding, RFC 8188).
 *
 * Push services (FCM, Mozilla autopush, Apple) only deliver a payload that is
 * encrypted to the subscription's `p256dh` key and `auth` secret. A push sent
 * with a plain JSON body is rejected, so every notification this worker sends
 * goes through `encryptWebPush`. WebCrypto only, so it runs in Workers and in
 * Node (the RFC test vector is in webpush.test.ts).
 */

const RECORD_SIZE = 4096;
const enc = new TextEncoder();

export function base64UrlToBytes(value: string): Uint8Array {
  const padded = value.replace(/-/g, "+").replace(/_/g, "/") + "===".slice((value.length + 3) % 4);
  const binary = atob(padded);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

export function bytesToBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=/g, "");
}

/** WebCrypto takes BufferSource; newer TS types Uint8Array by its buffer, so say it once here. */
const buf = (u: Uint8Array): BufferSource => u as unknown as BufferSource;

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.byteLength, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.byteLength;
  }
  return out;
}

async function hkdf(salt: Uint8Array, ikm: Uint8Array, info: Uint8Array, length: number): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey("raw", buf(ikm), "HKDF", false, ["deriveBits"]);
  const bits = await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: buf(salt), info: buf(info) }, key, length * 8);
  return new Uint8Array(bits);
}

/** An application server key pair for one message: the private key and its uncompressed public point. */
export interface ServerKeys {
  privateKey: CryptoKey;
  publicRaw: Uint8Array;
}

async function freshServerKeys(): Promise<ServerKeys> {
  const pair = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair;
  const publicRaw = new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer);
  return { privateKey: pair.privateKey, publicRaw };
}

/**
 * Encrypt `plaintext` for one push subscription. `p256dh` and `auth` are the
 * subscription's keys (base64url). `salt` and `serverKeys` are fixed only in
 * tests; production makes new ones per message, as the RFC requires.
 */
export async function encryptWebPush(
  plaintext: Uint8Array,
  p256dh: string,
  auth: string,
  opts: { salt?: Uint8Array; serverKeys?: ServerKeys } = {},
): Promise<Uint8Array> {
  const uaPublic = base64UrlToBytes(p256dh);
  const authSecret = base64UrlToBytes(auth);
  if (uaPublic.byteLength !== 65 || uaPublic[0] !== 0x04) throw new Error("invalid p256dh key");
  if (authSecret.byteLength < 16) throw new Error("invalid auth secret");
  if (plaintext.byteLength > RECORD_SIZE - 16 - 1 - 86) throw new Error("push payload too large");

  const salt = opts.salt ?? crypto.getRandomValues(new Uint8Array(16));
  const server = opts.serverKeys ?? (await freshServerKeys());
  const uaKey = await crypto.subtle.importKey("raw", buf(uaPublic), { name: "ECDH", namedCurve: "P-256" }, false, []);
  // Workers' older type names call this field `$public`; the runtime (and the spec) take `public`.
  const ecdhParams = { name: "ECDH", public: uaKey } as unknown as Parameters<typeof crypto.subtle.deriveBits>[0];
  const ecdhSecret = new Uint8Array(await crypto.subtle.deriveBits(ecdhParams, server.privateKey, 256));

  const keyInfo = concat(enc.encode("WebPush: info\0"), uaPublic, server.publicRaw);
  const ikm = await hkdf(authSecret, ecdhSecret, keyInfo, 32);
  const cek = await hkdf(salt, ikm, enc.encode("Content-Encoding: aes128gcm\0"), 16);
  const nonce = await hkdf(salt, ikm, enc.encode("Content-Encoding: nonce\0"), 12);

  const aesKey = await crypto.subtle.importKey("raw", buf(cek), "AES-GCM", false, ["encrypt"]);
  // One record: the plaintext, then the last-record delimiter 0x02 (no padding).
  const record = concat(plaintext, new Uint8Array([0x02]));
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: buf(nonce) }, aesKey, buf(record)));

  const header = new Uint8Array(16 + 4 + 1 + server.publicRaw.byteLength);
  header.set(salt, 0);
  new DataView(header.buffer).setUint32(16, RECORD_SIZE);
  header[20] = server.publicRaw.byteLength;
  header.set(server.publicRaw, 21);
  return concat(header, ciphertext);
}

/** Import an uncompressed P-256 key pair (tests: the RFC 8291 vector). */
export async function importServerKeys(publicB64: string, privateB64: string): Promise<ServerKeys> {
  const publicRaw = base64UrlToBytes(publicB64);
  const jwk: JsonWebKey = {
    kty: "EC",
    crv: "P-256",
    x: bytesToBase64Url(publicRaw.slice(1, 33)),
    y: bytesToBase64Url(publicRaw.slice(33, 65)),
    d: privateB64,
    ext: true,
  };
  const privateKey = await crypto.subtle.importKey("jwk", jwk, { name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
  return { privateKey, publicRaw };
}

export interface NotificationAction {
  action: string;
  title: string;
}

/** At most two buttons, each a short `{ action, title }`; anything else is dropped. */
export function sanitizeActions(value: unknown): NotificationAction[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const out: NotificationAction[] = [];
  for (const a of value) {
    if (!a || typeof a !== "object") continue;
    const { action, title } = a as Record<string, unknown>;
    if (typeof action !== "string" || typeof title !== "string") continue;
    if (!action || action.length > 32 || !title || title.length > 40) continue;
    out.push({ action, title });
    if (out.length === 2) break;
  }
  return out.length ? out : undefined;
}

const MAX_DATA_BYTES = 2048;

/** A flat object of strings, numbers and booleans, small enough for a push; else undefined. */
export function sanitizeData(value: unknown): Record<string, string | number | boolean> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const out: Record<string, string | number | boolean> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") out[k] = v;
  }
  if (enc.encode(JSON.stringify(out)).byteLength > MAX_DATA_BYTES) return undefined;
  return Object.keys(out).length ? out : undefined;
}
