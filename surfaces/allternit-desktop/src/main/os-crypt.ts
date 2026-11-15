/**
 * macOS Keychain-backed OSCrypt facade. On darwin the Keychain password is read
 * by a native addon off the main thread, the key is derived here, and
 * encrypt/decrypt stay synchronous once unlocked. Format is byte-compatible with
 * Chromium mac OSCrypt (v10, AES-128-CBC, IV of 16 spaces). Other platforms and
 * a missing addon delegate to electron safeStorage.
 */
import { createCipheriv, createDecipheriv, pbkdf2Sync, randomBytes } from 'node:crypto';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import log from 'electron-log';

export type OsCryptState = 'idle' | 'pending' | 'ready' | 'denied' | 'error';

export class KeychainUnavailableError extends Error {
  reason: 'pending' | 'denied' | 'error';
  constructor(reason: 'pending' | 'denied' | 'error', message?: string) {
    super(message ?? `Keychain unavailable: ${reason}`);
    this.name = 'KeychainUnavailableError';
    this.reason = reason;
  }
}

export type KeychainAddon = {
  getPassword(service: string, account: string): Promise<
    | { status: 'ok'; password: Buffer }
    | { status: 'not_found' }
    | { status: 'denied' | 'error'; code: number }
  >;
  addPassword(service: string, account: string, password: string | Buffer): Promise<
    { status: 'ok' | 'duplicate' } | { status: 'error'; code: number }
  >;
};

type SafeStorageLike = {
  isEncryptionAvailable(): boolean;
  encryptString(s: string): Buffer;
  decryptString(b: Buffer): string;
};

const require_ = createRequire(import.meta.url);
const here = path.dirname(fileURLToPath(import.meta.url));

let state: OsCryptState = 'idle';
let inflight: Promise<'ready' | 'unavailable'> | null = null;
let key: Buffer | null = null;
let useSafeStorage = false;

// Testing seam only.
let override: { loadAddon?: () => KeychainAddon; appName?: string; platform?: NodeJS.Platform } = {};
export function __setOsCryptTestSeam(o: typeof override | null): void {
  override = o ?? {};
  state = 'idle';
  inflight = null;
  key = null;
  useSafeStorage = false;
}

function electronApi(): { app: any; safeStorage: SafeStorageLike } {
  return require_('electron');
}
function platform(): NodeJS.Platform {
  return override.platform ?? process.platform;
}
function appName(): string {
  return override.appName ?? electronApi().app.getName();
}

function loadAddon(): KeychainAddon {
  if (override.loadAddon) return override.loadAddon();
  const { app } = electronApi();
  const file = app.isPackaged
    ? path.join(process.resourcesPath, 'native', 'keychain', 'keychain.node')
    : path.resolve(here, '../../native/keychain/build/keychain.node');
  return require_(file) as KeychainAddon;
}

function deriveKey(password: Buffer): Buffer {
  return pbkdf2Sync(password, 'saltysalt', 1003, 16, 'sha1');
}

async function unlock(): Promise<'ready' | 'unavailable'> {
  let addon: KeychainAddon;
  try {
    addon = loadAddon();
  } catch (err) {
    log.error('[Keychain] addon failed to load; falling back to safeStorage', err);
    useSafeStorage = true;
    state = 'ready';
    return 'ready';
  }
  const service = `${appName()} Safe Storage`;
  const account = `${appName()} Key`;
  try {
    let res = await addon.getPassword(service, account);
    if (res.status === 'not_found') {
      const created = await addon.addPassword(service, account, randomBytes(16).toString('base64'));
      if (created.status === 'error') {
        state = 'error';
        return 'unavailable';
      }
      res = await addon.getPassword(service, account);
    }
    if (res.status === 'ok') {
      key = deriveKey(res.password);
      state = 'ready';
      return 'ready';
    }
    state = res.status === 'denied' ? 'denied' : 'error';
    if (res.status !== 'denied') log.error('[Keychain] read failed', res);
    return 'unavailable';
  } catch (err) {
    log.error('[Keychain] unlock threw', err);
    state = 'error';
    return 'unavailable';
  }
}

export function osCryptReady(): Promise<'ready' | 'unavailable'> {
  if (platform() !== 'darwin') {
    useSafeStorage = true;
    state = 'ready';
    return Promise.resolve('ready');
  }
  if (state === 'ready') return Promise.resolve('ready');
  if (inflight) return inflight;
  state = 'pending';
  inflight = unlock().finally(() => {
    inflight = null;
  });
  return inflight;
}

export function osCryptState(): OsCryptState {
  return state;
}

function requireKey(): Buffer {
  if (key) return key;
  const reason = state === 'denied' ? 'denied' : state === 'error' ? 'error' : 'pending';
  throw new KeychainUnavailableError(reason);
}

export function isEncryptionAvailable(): boolean {
  if (platform() !== 'darwin' || useSafeStorage) return electronApi().safeStorage.isEncryptionAvailable();
  return key !== null;
}

export function encryptString(plain: string): Buffer {
  if (platform() !== 'darwin' || useSafeStorage) return electronApi().safeStorage.encryptString(plain);
  const c = createCipheriv('aes-128-cbc', requireKey(), Buffer.alloc(16, 0x20));
  return Buffer.concat([Buffer.from('v10'), c.update(plain, 'utf8'), c.final()]);
}

export function decryptString(buf: Buffer): string {
  if (platform() !== 'darwin' || useSafeStorage) return electronApi().safeStorage.decryptString(buf);
  const k = requireKey();
  if (buf.length < 3 || buf.subarray(0, 3).toString('latin1') !== 'v10') {
    throw new Error('os-crypt: unsupported ciphertext (missing v10 prefix)');
  }
  const d = createDecipheriv('aes-128-cbc', k, Buffer.alloc(16, 0x20));
  return Buffer.concat([d.update(buf.subarray(3)), d.final()]).toString('utf8');
}
