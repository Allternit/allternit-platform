// Happy wire crypto, reimplemented from slopus/happy-cli @ c2bacb3:
//   src/api/encryption.ts  (encryptLegacy/decryptLegacy, encryptWithDataKey/decryptWithDataKey,
//                           libsodiumPublicKeyFromSecretKey, libsodiumEncryptForPublicKey, authChallenge)
//   src/utils/deriveKey.ts (HMAC-SHA512 key tree)
//   src/api/api.ts         (dataKey session keys sealed to the account public key)
// Legacy accounts: the 32-byte account secret is the session key (tweetnacl secretbox).
// Data-key accounts: each session has a random 32-byte AES-256-GCM key, sealed to the account's
// content public key (tweetnacl box, ephemeral sender key, 24-byte nonce prepended).
import { createHash, createHmac, randomBytes, createCipheriv, createDecipheriv } from 'node:crypto';
import nacl from 'tweetnacl';

export const DEFAULT_SERVER_URL = 'https://api.cluster-fluster.com';

export type Json = unknown;

export function b64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString('base64');
}

export function unb64(text: string): Uint8Array {
  return new Uint8Array(Buffer.from(text, 'base64'));
}

/** Accepts the base64url form the app shows as a restore secret, and plain base64. */
export function parseSecret(text: string): Uint8Array | null {
  const normalized = text.trim().replace(/-/g, '+').replace(/_/g, '/');
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(normalized)) return null;
  const bytes = unb64(normalized);
  return bytes.length === 32 ? bytes : null;
}

/** Challenge/response login: POST /v1/auth. Signature is ed25519 over 32 random bytes. */
export function authChallenge(secret: Uint8Array) {
  const keyPair = nacl.sign.keyPair.fromSeed(secret);
  const challenge = new Uint8Array(randomBytes(32));
  const signature = nacl.sign.detached(challenge, keyPair.secretKey);
  return { challenge: b64(challenge), publicKey: b64(keyPair.publicKey), signature: b64(signature) };
}

/** HMAC-SHA512 key tree, as deriveKey(master, usage, path) in happy-cli. */
export function deriveKey(master: Uint8Array, usage: string, path: string[]): Uint8Array {
  const root = createHmac('sha512', Buffer.from(usage + ' Master Seed', 'utf8')).update(master).digest();
  let key = root.subarray(0, 32);
  let chain = root.subarray(32);
  for (const index of path) {
    const I = createHmac('sha512', chain).update(Buffer.concat([Buffer.from([0x0]), Buffer.from(index, 'utf8')])).digest();
    key = I.subarray(0, 32);
    chain = I.subarray(32);
  }
  return new Uint8Array(key);
}

/** Box secret key for the account's content key pair (libsodium-style: sha512(seed)[0:32]). */
export function contentBoxSecret(secret: Uint8Array): Uint8Array {
  const seed = deriveKey(secret, 'Happy EnCoder', ['content']);
  return new Uint8Array(createHash('sha512').update(seed).digest().subarray(0, 32));
}

/** Unseal bundle: ephemeralPk(32) | nonce(24) | box. Mirrors libsodiumEncryptForPublicKey. */
export function openSealed(bundle: Uint8Array, recipientSecret: Uint8Array): Uint8Array | null {
  if (bundle.length < 32 + 24 + nacl.box.overheadLength) return null;
  const ephemeralPk = bundle.subarray(0, 32);
  const nonce = bundle.subarray(32, 56);
  const boxed = bundle.subarray(56);
  return nacl.box.open(boxed, nonce, ephemeralPk, recipientSecret) ?? null;
}

/** Seal bytes to a recipient public key, same layout as libsodiumEncryptForPublicKey (tests only). */
export function sealFor(data: Uint8Array, recipientPublicKey: Uint8Array): Uint8Array {
  const eph = nacl.box.keyPair();
  const nonce = new Uint8Array(randomBytes(24));
  const boxed = nacl.box(data, nonce, recipientPublicKey, eph.secretKey);
  const out = new Uint8Array(32 + 24 + boxed.length);
  out.set(eph.publicKey, 0);
  out.set(nonce, 32);
  out.set(boxed, 56);
  return out;
}

export type SessionCipher = { variant: 'legacy' | 'dataKey'; key: Uint8Array };

/** Per-session key: legacy uses the account secret; dataKey unseals the session's dataEncryptionKey. */
export function sessionCipher(secret: Uint8Array, dataEncryptionKeyB64: string | null): SessionCipher | null {
  if (!dataEncryptionKeyB64) return { variant: 'legacy', key: secret };
  const bundle = unb64(dataEncryptionKeyB64);
  if (bundle[0] !== 0) return null;
  const key = openSealed(bundle.subarray(1), contentBoxSecret(secret));
  return key && key.length === 32 ? { variant: 'dataKey', key } : null;
}

export function encryptWith(cipher: SessionCipher, data: Json): Uint8Array {
  const plaintext = new TextEncoder().encode(JSON.stringify(data));
  if (cipher.variant === 'legacy') {
    const nonce = new Uint8Array(randomBytes(nacl.secretbox.nonceLength));
    const boxed = nacl.secretbox(plaintext, nonce, cipher.key);
    const out = new Uint8Array(nonce.length + boxed.length);
    out.set(nonce, 0);
    out.set(boxed, nonce.length);
    return out;
  }
  const nonce = randomBytes(12);
  const aes = createCipheriv('aes-256-gcm', cipher.key, nonce);
  const body = Buffer.concat([aes.update(plaintext), aes.final()]);
  const tag = aes.getAuthTag();
  return new Uint8Array(Buffer.concat([Buffer.from([0]), nonce, body, tag]));
}

export function decryptWith(cipher: SessionCipher, bundle: Uint8Array): Json | null {
  try {
    if (cipher.variant === 'legacy') {
      const nonce = bundle.subarray(0, nacl.secretbox.nonceLength);
      const boxed = bundle.subarray(nacl.secretbox.nonceLength);
      const opened = nacl.secretbox.open(boxed, nonce, cipher.key);
      return opened ? JSON.parse(new TextDecoder().decode(opened)) : null;
    }
    if (bundle.length < 1 + 12 + 16 || bundle[0] !== 0) return null;
    const nonce = bundle.subarray(1, 13);
    const tag = bundle.subarray(bundle.length - 16);
    const body = bundle.subarray(13, bundle.length - 16);
    const aes = createDecipheriv('aes-256-gcm', cipher.key, nonce);
    aes.setAuthTag(tag);
    const plain = Buffer.concat([aes.update(body), aes.final()]);
    return JSON.parse(plain.toString('utf8'));
  } catch {
    return null;
  }
}
