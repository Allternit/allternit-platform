/**
 * Minimal secure store for desktop-main secrets (managed runtime, P1).
 *
 * Single-purpose: holds the fabric-transport worker credential the desktop
 * provisions on first launch. Follows the auth-manager secret pattern:
 * in packaged builds OS encryption (os-crypt.ts: the macOS Keychain read off
 * the main thread, Electron safeStorage elsewhere) is the sole Keychain owner;
 * development builds fall back to an authenticated-local AES-GCM envelope so
 * rebuilding the API or Electron does not cause repeated macOS Keychain
 * prompts. Values are encrypted at rest (mode 0600 inside a 0700 directory);
 * the plaintext never leaves the main process.
 *
 * A Keychain that is still unlocking, denied or failing is never mistaken for
 * a missing or damaged secret: KeychainUnavailableError propagates, and a
 * Keychain-sealed file is never overwritten with a local envelope.
 */

import { app } from 'electron';
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import {
  decryptString,
  encryptString,
  isEncryptionAvailable,
  KeychainUnavailableError,
  osCryptReady,
  osCryptState,
} from './os-crypt.js';

export const SAFE_STORAGE_HEADER = 'allternit-safe-storage-v1\n';
const LOCAL_STORAGE_HEADER = 'allternit-local-secret-v1\n';

/**
 * Why OS encryption can't be used right now because of the macOS Keychain, or
 * null when it can (or the platform has no Keychain gate). Off macOS, and when
 * the facade fell back to safeStorage, unavailability keeps today's behaviour.
 */
export function keychainUnavailable(): KeychainUnavailableError | null {
  if (process.platform !== 'darwin' || isEncryptionAvailable()) return null;
  const state = osCryptState();
  if (state === 'ready') return null;
  return new KeychainUnavailableError(state === 'denied' ? 'denied' : state === 'error' ? 'error' : 'pending');
}

function raise(error: KeychainUnavailableError): never {
  // Lazy unlock on first use (dev builds have no startup gate); single-flight.
  if (error.reason === 'pending') void osCryptReady();
  throw error;
}

/**
 * Await the Keychain unlock. After a definite denial or error this does not
 * prompt again (matching Chromium, which caches the failure per launch); the
 * startup dialog's "Try again" is the retry.
 */
export async function keychainSettled(): Promise<void> {
  const state = osCryptState();
  if (state === 'denied' || state === 'error') return;
  await osCryptReady();
}

/**
 * For stores that use OS encryption only in packaged builds (dev uses the
 * local envelope): packaged builds wait for the Keychain; dev builds wait only
 * when one of `files` is Keychain-sealed (lazy unlock, no new dev prompts).
 */
export async function secretsReady(...files: string[]): Promise<void> {
  if (app.isPackaged || files.some(isSafeStorageFile)) await keychainSettled();
}

/** True when the file starts with the OS-encryption (safe-storage) header. */
export function isSafeStorageFile(file: string): boolean {
  let fd: number | null = null;
  try {
    fd = fs.openSync(file, 'r');
    const head = Buffer.alloc(SAFE_STORAGE_HEADER.length);
    const read = fs.readSync(fd, head, 0, head.length, 0);
    return read === head.length && head.toString('utf8') === SAFE_STORAGE_HEADER;
  } catch {
    return false;
  } finally {
    if (fd !== null) fs.closeSync(fd);
  }
}

/** `${SAFE_STORAGE_HEADER}<base64 ciphertext>`; caller checked isEncryptionAvailable(). */
export function sealWithOsEncryption(value: string): Buffer {
  return Buffer.from(`${SAFE_STORAGE_HEADER}${encryptString(value).toString('base64')}`, 'utf8');
}

/** Opens a value that starts with SAFE_STORAGE_HEADER. Throws KeychainUnavailableError when the Keychain is not unlocked. */
export function openOsEncrypted(value: string): string {
  const blocked = keychainUnavailable();
  if (blocked) raise(blocked);
  if (!isEncryptionAvailable()) throw new Error('macOS credential storage is unavailable');
  return decryptString(Buffer.from(value.slice(SAFE_STORAGE_HEADER.length), 'base64'));
}

/**
 * Call before writing a local-envelope copy because OS encryption is not
 * usable. Throws KeychainUnavailableError when that would only be because the
 * Keychain is unlocking (packaged builds never downgrade then), or would
 * overwrite a Keychain-sealed file while the Keychain is pending, denied or
 * failing. A denial with nothing sealed on disk keeps today's degraded mode.
 */
export function assertLocalEnvelopeAllowed(target: string): void {
  const blocked = keychainUnavailable();
  if (!blocked) return;
  if ((app.isPackaged && blocked.reason === 'pending') || isSafeStorageFile(target)) raise(blocked);
}

/**
 * True when anything in userData is sealed with OS encryption, so continuing
 * without the Keychain would lose access to it. Used by the startup gate.
 */
export function hasKeychainSealedSecrets(userData = app.getPath('userData')): boolean {
  const sealedIn = (dir: string, exts: string[]) => {
    try {
      return fs
        .readdirSync(dir)
        .some((name) => exts.some((ext) => name.endsWith(ext)) && isSafeStorageFile(path.join(dir, name)));
    } catch {
      return false;
    }
  };
  const nonEmptyJson = (file: string, test: (data: unknown) => boolean) => {
    try {
      return test(JSON.parse(fs.readFileSync(file, 'utf8')));
    } catch {
      return false;
    }
  };
  const hasValues = (data: unknown) =>
    !!data && typeof data === 'object' &&
    Object.values(data as Record<string, unknown>).some((v) =>
      typeof v === 'string' ? v.length > 0 : !!v && typeof v === 'object' && Object.keys(v).length > 0);
  return (
    sealedIn(path.join(userData, 'auth'), ['.bin', '.json']) ||
    sealedIn(path.join(userData, 'secrets'), ['.bin']) ||
    isSafeStorageFile(path.join(userData, 'machine-id.bin')) ||
    nonEmptyJson(path.join(userData, 'mini-app-secrets.json'), hasValues) ||
    nonEmptyJson(path.join(userData, 'mini-app-oauth-tokens.json'), hasValues) ||
    nonEmptyJson(path.join(userData, 'phone', 'usb-agents.json'), (data) =>
      Array.isArray(data) && data.some((row) => typeof row?.sealedKey === 'string' && row.sealedKey.startsWith('v1:')))
  );
}

function storePath(key: string): string {
  const safe = key.replace(/[^a-z0-9_-]/gi, '_');
  return path.join(app.getPath('userData'), 'secrets', `${safe}.bin`);
}

function localHardwareKey(): Buffer {
  // Same inputs as auth-manager's localHardwareKey (hostname deliberately
  // excluded — it flips between mDNS names on macOS).
  return crypto
    .createHash('sha256')
    .update(`allternit-desktop-local-secret|${app.getPath('userData')}`)
    .digest();
}

function encodeSecret(value: string, target: string): Buffer {
  if (app.isPackaged && isEncryptionAvailable()) return sealWithOsEncryption(value);
  assertLocalEnvelopeAllowed(target);
  const key = localHardwareKey();
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  const ciphertext = Buffer.concat([cipher.update(value, 'utf8'), cipher.final()]);
  return Buffer.from(
    `${LOCAL_STORAGE_HEADER}${Buffer.concat([iv, cipher.getAuthTag(), ciphertext]).toString('base64')}`,
    'utf8',
  );
}

function decodeSecret(raw: Buffer): string {
  const value = raw.toString('utf8');
  if (value.startsWith(SAFE_STORAGE_HEADER)) return openOsEncrypted(value);
  if (value.startsWith(LOCAL_STORAGE_HEADER)) {
    const payload = Buffer.from(value.slice(LOCAL_STORAGE_HEADER.length), 'base64');
    const iv = payload.subarray(0, 12);
    const tag = payload.subarray(12, 28);
    const ciphertext = payload.subarray(28);
    const decipher = crypto.createDecipheriv('aes-256-gcm', localHardwareKey(), iv);
    decipher.setAuthTag(tag);
    return Buffer.concat([decipher.update(ciphertext), decipher.final()]).toString('utf8');
  }
  throw new Error('Unknown secure-store entry format');
}

/**
 * Read a secret. Returns null when the entry does not exist or is damaged.
 * Throws KeychainUnavailableError when the Keychain is not unlocked: callers
 * must not read that as "no secret" (and re-provision or re-pair).
 */
export function readSecret(key: string): string | null {
  const file = storePath(key);
  if (!fs.existsSync(file)) return null;
  try {
    return decodeSecret(fs.readFileSync(file));
  } catch (error) {
    if (error instanceof KeychainUnavailableError) throw error;
    return null;
  }
}

/** Write (or overwrite) a secret, encrypted at rest. */
export function writeSecret(key: string, value: string): void {
  const file = storePath(key);
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  fs.writeFileSync(file, encodeSecret(value, file), { mode: 0o600 });
}

/** Delete a secret if present. */
export function deleteSecret(key: string): void {
  const file = storePath(key);
  if (fs.existsSync(file)) fs.rmSync(file);
}

/** Key for the fabric-transport worker credential in the Keychain store. */
export const FABRIC_WORKER_TOKEN_KEY = 'fabric-worker-token';
