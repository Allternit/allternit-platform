/**
 * Phone Link pairing handover (Eoj, 2026-10-17): a phone linked on the Mac is
 * given to the person's Sessions computer, so that computer's Desktop serves
 * the phone's apps with the Mac off. The pairing already exists; this moves
 * it. It never re-pairs: the phone helpers refuse pairing over the mesh on
 * purpose, and approving a new computer on the phone itself is a future note
 * only (not built).
 *
 * What moves: the link record (non-secret) and the phone's iCTRL key record
 * (secret-store entry `phone-link.ictrl.<device>`). The Mac keeps its copy —
 * an iPhone still needs the Mac and a cable to re-sign the helper and to
 * remount the developer image after a restart.
 *
 * Path (each hop authenticated, the payload sealed end to end):
 *   Mac Desktop main → Mac allternit-api /api/v1/subscriptions/sessions-computer/phone-handover
 *   → the person's bound, running Sessions computer only (never a local
 *     gateway, never another owner's computer: the binding is per user)
 *   → its subscription gateway /v1/phone/handover (binding token)
 *   → its Desktop's loopback phone gateway /phone/handover (per-launch token)
 *   → importHandover() here.
 *
 * Sealing: the receiving Desktop keeps an X25519 key pair (private half in its
 * secure store). The Mac seals to the public half with an ephemeral X25519
 * key, HKDF-SHA256 and AES-256-GCM; the owner and the recipient key id are
 * bound in as associated data, so a box made for one computer or owner never
 * opens on another. Electron-free and unit-tested; phone-service.ts wires it.
 */

import * as crypto from 'node:crypto';
import type { SecretStore } from './ictrl-client.js';
import { ictrlStoreKey } from './ictrl-client.js';
import type { IosLinkRecord, IosLinkStore } from './phone-link-ios.js';
import type { AndroidLinkRecord, AndroidLinkStore } from './phone-link-android.js';
import type { HandedOverFrom, PhoneLinkHandoverResult, PhoneLinkHandoverTarget, PhoneLinkPlatform } from './phone-link-types.js';

export const HANDOVER_VERSION = 1;
/** Secure-store entry for this Desktop's handover key pair (PKCS#8, base64). */
export const HANDOVER_KEY_ENTRY = 'phone-link.handover-key';
const HKDF_INFO = Buffer.from('allternit-phone-handover-v1');

export type { HandedOverFrom };

export type HandoverPayload =
  | { v: 1; platform: 'ios'; record: IosLinkRecord; secret: string; from: HandedOverFrom }
  | { v: 1; platform: 'android'; record: AndroidLinkRecord; secret: string; from: HandedOverFrom };

/** What the receiving Desktop publishes: its public key and, when signed in, its owner. */
export interface HandoverKey {
  v: 1;
  /** X25519 public key, SPKI DER, base64. */
  publicKey: string;
  keyId: string;
  /** The signed-in Allternit user on the receiving Desktop, null when unknown. */
  owner: string | null;
}

export interface SealedHandover {
  v: 1;
  keyId: string;
  /** The sender's owner, bound into the AAD. */
  owner: string;
  /** Ephemeral X25519 public key, SPKI DER, base64. */
  epk: string;
  iv: string;
  ct: string;
  tag: string;
}

export class HandoverError extends Error {
  constructor(
    readonly code: 'not_linked' | 'no_secret' | 'wrong_owner' | 'wrong_key' | 'bad_box' | 'bad_payload',
    message: string,
  ) {
    super(message);
    this.name = 'HandoverError';
  }
}

export interface HandoverStores {
  iosStore: IosLinkStore;
  androidStore: AndroidLinkStore;
  secrets: SecretStore;
}

function keyIdOf(publicKeyDerB64: string): string {
  return crypto.createHash('sha256').update(Buffer.from(publicKeyDerB64, 'base64')).digest('hex').slice(0, 16);
}

function aad(keyId: string, owner: string): Buffer {
  return Buffer.from(JSON.stringify({ v: HANDOVER_VERSION, keyId, owner }));
}

function derive(shared: Buffer, epk: string, keyId: string): Buffer {
  return Buffer.from(crypto.hkdfSync('sha256', shared, Buffer.from(`${epk}|${keyId}`), HKDF_INFO, 32));
}

// ── Receiving side: the key pair ────────────────────────────────────────────

/** This Desktop's handover key (created on first use, private half in the secure store). */
export function handoverKey(secrets: SecretStore, owner: string | null): HandoverKey {
  let pkcs8 = secrets.get(HANDOVER_KEY_ENTRY);
  let privateKey: crypto.KeyObject | null = null;
  if (pkcs8) {
    try {
      privateKey = crypto.createPrivateKey({ key: Buffer.from(pkcs8, 'base64'), format: 'der', type: 'pkcs8' });
    } catch {
      privateKey = null;
    }
  }
  if (!privateKey || privateKey.asymmetricKeyType !== 'x25519') {
    const pair = crypto.generateKeyPairSync('x25519');
    privateKey = pair.privateKey;
    pkcs8 = privateKey.export({ format: 'der', type: 'pkcs8' }).toString('base64');
    secrets.set(HANDOVER_KEY_ENTRY, pkcs8);
  }
  const publicKey = crypto.createPublicKey(privateKey).export({ format: 'der', type: 'spki' }).toString('base64');
  return { v: HANDOVER_VERSION, publicKey, keyId: keyIdOf(publicKey), owner };
}

// ── Giving side ─────────────────────────────────────────────────────────────

/** One linked phone's pairing, read from this Desktop's stores. */
export function exportPairing(stores: HandoverStores, deviceId: string, from: HandedOverFrom): HandoverPayload {
  const ios = stores.iosStore.all().find((r) => (r.deviceId ?? r.udid) === deviceId || r.udid === deviceId);
  if (ios) {
    const secret = stores.secrets.get(ictrlStoreKey(ios.udid));
    if (!secret) throw new HandoverError('no_secret', 'This computer has no key for that iPhone. Set it up again first.');
    // A phone this computer itself received is passed on as its own record.
    const { handedOverFrom: _drop, ...record } = ios;
    return { v: HANDOVER_VERSION, platform: 'ios', record, secret, from };
  }
  const android = stores.androidStore.load(deviceId);
  if (android) {
    const secret = stores.secrets.get(ictrlStoreKey(`android:${android.deviceId}`));
    if (!secret) throw new HandoverError('no_secret', 'This computer has no key for that Android phone. Set it up again first.');
    const { handedOverFrom: _drop, ...record } = android;
    return { v: HANDOVER_VERSION, platform: 'android', record, secret, from };
  }
  throw new HandoverError('not_linked', 'That phone isn’t linked on this computer.');
}

/**
 * Seal a payload to the receiver's key. Refuses when the receiver reports a
 * different signed-in owner than ours (defense in depth; the API path only
 * reaches the caller's own Sessions computer).
 */
export function sealHandover(payload: HandoverPayload, recipient: HandoverKey, owner: string): SealedHandover {
  if (recipient.owner && recipient.owner !== owner) {
    throw new HandoverError('wrong_owner', 'That computer is signed in to a different Allternit account.');
  }
  if (keyIdOf(recipient.publicKey) !== recipient.keyId) throw new HandoverError('wrong_key', 'The Sessions computer sent a key that doesn’t match its id.');
  const peer = crypto.createPublicKey({ key: Buffer.from(recipient.publicKey, 'base64'), format: 'der', type: 'spki' });
  const eph = crypto.generateKeyPairSync('x25519');
  const epk = eph.publicKey.export({ format: 'der', type: 'spki' }).toString('base64');
  const key = derive(crypto.diffieHellman({ privateKey: eph.privateKey, publicKey: peer }), epk, recipient.keyId);
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  cipher.setAAD(aad(recipient.keyId, owner));
  const ct = Buffer.concat([cipher.update(JSON.stringify(payload), 'utf8'), cipher.final()]);
  return {
    v: HANDOVER_VERSION,
    keyId: recipient.keyId,
    owner,
    epk,
    iv: iv.toString('base64'),
    ct: ct.toString('base64'),
    tag: cipher.getAuthTag().toString('base64'),
  };
}

// ── Receiving side: open + import ───────────────────────────────────────────

function isPayload(v: unknown): v is HandoverPayload {
  const p = v as Partial<HandoverPayload> | null;
  if (!p || p.v !== HANDOVER_VERSION || typeof p.secret !== 'string' || !p.record || typeof p.from?.name !== 'string') return false;
  if (p.platform === 'ios') return typeof (p.record as IosLinkRecord).udid === 'string';
  if (p.platform === 'android') return typeof (p.record as AndroidLinkRecord).deviceId === 'string';
  return false;
}

/** Open a sealed box with this Desktop's key. `owner`: our signed-in user (null = unknown). */
export function openHandover(sealed: SealedHandover, secrets: SecretStore, owner: string | null): HandoverPayload {
  const mine = handoverKey(secrets, owner);
  if (sealed?.v !== HANDOVER_VERSION || sealed.keyId !== mine.keyId) {
    throw new HandoverError('wrong_key', 'This pairing was sealed for a different computer.');
  }
  if (owner && sealed.owner !== owner) throw new HandoverError('wrong_owner', 'This pairing belongs to a different Allternit account.');
  const privateKey = crypto.createPrivateKey({ key: Buffer.from(secrets.get(HANDOVER_KEY_ENTRY)!, 'base64'), format: 'der', type: 'pkcs8' });
  let plain: string;
  try {
    const epk = crypto.createPublicKey({ key: Buffer.from(sealed.epk, 'base64'), format: 'der', type: 'spki' });
    const key = derive(crypto.diffieHellman({ privateKey, publicKey: epk }), sealed.epk, sealed.keyId);
    const decipher = crypto.createDecipheriv('aes-256-gcm', key, Buffer.from(sealed.iv, 'base64'));
    decipher.setAAD(aad(sealed.keyId, sealed.owner));
    decipher.setAuthTag(Buffer.from(sealed.tag, 'base64'));
    plain = Buffer.concat([decipher.update(Buffer.from(sealed.ct, 'base64')), decipher.final()]).toString('utf8');
  } catch {
    throw new HandoverError('bad_box', 'The pairing could not be opened; it was changed on the way or sealed for another computer.');
  }
  let payload: unknown;
  try {
    payload = JSON.parse(plain);
  } catch {
    throw new HandoverError('bad_payload', 'The pairing was not readable.');
  }
  if (!isPayload(payload)) throw new HandoverError('bad_payload', 'The pairing is missing its phone record or key.');
  return payload;
}

/** Write an opened pairing into this Desktop's stores (the secret re-encrypted by the local secure store). */
export function importHandover(stores: HandoverStores, payload: HandoverPayload, now: () => Date = () => new Date()): { deviceId: string; platform: PhoneLinkPlatform; name?: string } {
  const from: HandedOverFrom = { name: payload.from.name, at: now().toISOString() };
  if (payload.platform === 'ios') {
    // The saved LAN host only helps on the same network; a stale one falls back to the scan and the mesh host.
    // Call audio names this computer's sound devices: another computer's settings never come along.
    const { callBridge: _otherComputersAudio, ...record } = payload.record;
    stores.iosStore.save({ ...record, handedOverFrom: from });
    stores.secrets.set(ictrlStoreKey(payload.record.udid), payload.secret);
    return { deviceId: payload.record.deviceId ?? payload.record.udid, platform: 'ios', name: payload.record.deviceName };
  }
  stores.androidStore.save({ ...payload.record, handedOverFrom: from });
  stores.secrets.set(ictrlStoreKey(`android:${payload.record.deviceId}`), payload.secret);
  return { deviceId: payload.record.deviceId, platform: 'android', name: payload.record.deviceName };
}

// ── Giving side: the API calls (phone-service.ts supplies Electron pieces) ──

/** allternit-api route (cmd/allternit-api/src/subscription_routes.rs): the caller's own Sessions computer only. */
export const HANDOVER_API_PATH = '/api/v1/subscriptions/sessions-computer/phone-handover';

/** GET HANDOVER_API_PATH: the bound Sessions computer and, when it can take a phone now, its key. */
export interface HandoverTargetResponse {
  computer: { id: string; name: string; running?: boolean } | null;
  ready: boolean;
  reason?: string;
  key?: HandoverKey;
}

export interface HandoverClientDeps {
  /** Local allternit-api base, no trailing slash. */
  apiUrl: () => string;
  /** Desktop access token + the signed-in user's headers. */
  headers: () => Record<string, string>;
  /** The signed-in Allternit user; handover needs one. */
  owner: () => string | null;
  stores: HandoverStores;
  /** This computer's name, shown on the receiving side ("Eoj's MacBook Pro"). */
  computerName: () => string;
  /** Ask the person on this computer. False = they said no. */
  confirm: (phone: { name: string; platform: PhoneLinkPlatform }, computerName: string) => Promise<boolean>;
  fetchImpl?: typeof fetch;
  now?: () => Date;
}

async function getTarget(deps: HandoverClientDeps): Promise<HandoverTargetResponse | { error: string; message: string }> {
  const f = deps.fetchImpl ?? fetch;
  try {
    const res = await f(`${deps.apiUrl()}${HANDOVER_API_PATH}`, { headers: deps.headers(), signal: AbortSignal.timeout(20_000) });
    const body = (await res.json().catch(() => null)) as HandoverTargetResponse | { error?: string; detail?: string } | null;
    if (!res.ok || !body || !('ready' in body)) {
      return { error: 'api_failed', message: (body as { detail?: string } | null)?.detail ?? `Allternit answered ${res.status}.` };
    }
    return body;
  } catch (e) {
    return { error: 'api_unreachable', message: `Couldn't reach Allternit: ${(e as Error).message}` };
  }
}

/** The computers this Desktop can hand a phone to: the person's bound Sessions computer, or none. */
export async function handoverTargets(deps: HandoverClientDeps): Promise<PhoneLinkHandoverTarget[]> {
  const t = await getTarget(deps);
  if ('error' in t || !t.computer) return [];
  return [{ computerId: t.computer.id, name: t.computer.name, ready: t.ready && !!t.key, ...(t.reason ? { reason: t.reason } : {}) }];
}

/** Give one linked phone's pairing to the Sessions computer, after the person confirms here. */
export async function handOver(deps: HandoverClientDeps, opts: { deviceId: string; computerId: string }): Promise<PhoneLinkHandoverResult> {
  const owner = deps.owner();
  if (!owner) return { ok: false, error: 'signed_out', message: 'Sign in to Allternit on this computer first.' };
  const target = await getTarget(deps);
  if ('error' in target) return { ok: false, ...target };
  if (!target.computer || target.computer.id !== opts.computerId) {
    return { ok: false, error: 'not_your_sessions_computer', message: 'That computer isn’t your Sessions computer any more.' };
  }
  if (!target.ready || !target.key) {
    return { ok: false, error: 'not_ready', message: target.reason ?? `${target.computer.name} can’t take a phone right now.` };
  }
  let payload: HandoverPayload;
  try {
    payload = exportPairing(deps.stores, opts.deviceId, { name: deps.computerName(), at: (deps.now ?? (() => new Date()))().toISOString() });
  } catch (e) {
    return { ok: false, error: e instanceof HandoverError ? e.code : 'export_failed', message: (e as Error).message };
  }
  const phoneName = payload.record.deviceName ?? (payload.platform === 'ios' ? 'iPhone' : 'Android phone');
  if (!(await deps.confirm({ name: phoneName, platform: payload.platform }, target.computer.name))) {
    return { ok: false, cancelled: true, error: 'cancelled', message: 'Not handed over.' };
  }
  let sealed: SealedHandover;
  try {
    sealed = sealHandover(payload, target.key, owner);
  } catch (e) {
    return { ok: false, error: e instanceof HandoverError ? e.code : 'seal_failed', message: (e as Error).message };
  }
  const f = deps.fetchImpl ?? fetch;
  try {
    const res = await f(`${deps.apiUrl()}${HANDOVER_API_PATH}`, {
      method: 'POST',
      headers: { ...deps.headers(), 'content-type': 'application/json' },
      body: JSON.stringify({ computer_id: opts.computerId, sealed }),
      signal: AbortSignal.timeout(60_000),
    });
    const body = (await res.json().catch(() => null)) as { ok?: boolean; deviceId?: string; error?: string; message?: string; detail?: string } | null;
    if (!res.ok || body?.ok !== true) {
      return { ok: false, error: body?.error ?? 'handover_failed', message: body?.message ?? body?.detail ?? `${target.computer.name} answered ${res.status}.` };
    }
    return { ok: true, deviceId: body.deviceId ?? opts.deviceId, computerName: target.computer.name };
  } catch (e) {
    return { ok: false, error: 'api_unreachable', message: `Couldn't reach ${target.computer.name}: ${(e as Error).message}` };
  }
}
