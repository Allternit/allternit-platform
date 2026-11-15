/**
 * Regression: a macOS Keychain that is pending, denied or failing must never be
 * read as a corrupt or missing secret. Before os-crypt, auth-manager
 * quarantined runtime-identity.json (forced re-pair) and replaced the connector
 * key, and secure-store answered null ("no key"), whenever decode threw,
 * including when the Keychain was merely unavailable.
 */
import * as fs from 'node:fs';
import * as path from 'node:path';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

// Created in beforeEach; hoisted so the electron mock (and the auth-manager
// singleton built at import) see it.
const env = vi.hoisted(() => ({
  userData: `${process.env.TMPDIR ?? '/tmp'}/keychain-unavailable-${process.pid}-${Date.now()}`,
}));

vi.mock('electron', () => ({
  app: { getPath: () => env.userData, isPackaged: true, getVersion: () => '0.0.0-test', quit: () => {} },
  ipcMain: { on: () => {}, handle: () => {} },
  BrowserWindow: class { static getAllWindows() { return []; } },
  powerMonitor: { on: () => {} },
  net: {},
  session: {},
  shell: { openExternal: () => {} },
}));

import { __setOsCryptTestSeam, encryptString, KeychainUnavailableError, osCryptReady } from './os-crypt.js';
import { DesktopAuthManager } from './auth-manager.js';
import { readSecret, SAFE_STORAGE_HEADER, writeSecret } from './secure-store.js';

const PASSWORD = Buffer.from('keychain-password');
const authDir = () => path.join(env.userData, 'auth');
const identityPath = () => path.join(authDir(), 'runtime-identity.json');
const platformKeyPath = () => path.join(authDir(), 'platform-encryption-key.bin');

function keychain(result: 'ok' | 'denied') {
  __setOsCryptTestSeam({
    platform: 'darwin',
    appName: 'Test',
    loadAddon: () => ({
      getPassword: async () => (result === 'ok' ? { status: 'ok', password: PASSWORD } : { status: 'denied', code: -128 }),
      addPassword: async () => ({ status: 'ok' }),
    }),
  });
}

async function sealed(value: string): Promise<Buffer> {
  keychain('ok');
  await osCryptReady();
  return Buffer.from(`${SAFE_STORAGE_HEADER}${encryptString(value).toString('base64')}`, 'utf8');
}

const identity = {
  runtimeId: 'rt_1',
  accessToken: 'tok',
  privateKeyPem: 'pem',
  userId: 'user_1',
  userEmail: 'a@example.com',
  expiresAt: Date.now() + 3_600_000,
};

const realPlatform = process.platform;
beforeAll(() => {
  Object.defineProperty(process, 'platform', { value: 'darwin' });
});
afterAll(() => {
  Object.defineProperty(process, 'platform', { value: realPlatform });
  fs.rmSync(env.userData, { recursive: true, force: true });
});

beforeEach(() => {
  fs.rmSync(env.userData, { recursive: true, force: true });
  fs.mkdirSync(authDir(), { recursive: true });
});

describe('Keychain unavailable is not corruption', () => {
  it('keeps the runtime identity when the Keychain is denied (no quarantine, no re-pair)', async () => {
    const raw = await sealed(JSON.stringify(identity));
    fs.writeFileSync(identityPath(), raw);
    keychain('denied');

    const manager = new DesktopAuthManager();
    await expect(manager.initialize()).rejects.toBeInstanceOf(KeychainUnavailableError);
    expect(fs.readFileSync(identityPath()).equals(raw)).toBe(true);
    expect(fs.readdirSync(authDir()).filter((f) => f.includes('.corrupt-'))).toEqual([]);
  });

  it('still quarantines an identity file that is genuinely damaged', async () => {
    keychain('ok');
    await osCryptReady();
    fs.writeFileSync(identityPath(), 'not an identity');

    const manager = new DesktopAuthManager();
    await manager.initialize();
    expect(manager.hasSession()).toBe(false);
    expect(fs.existsSync(identityPath())).toBe(false);
    expect(fs.readdirSync(authDir()).some((f) => f.startsWith('runtime-identity.json.corrupt-'))).toBe(true);
  });

  it('never replaces the sealed connector key while the Keychain is denied', async () => {
    const raw = await sealed('a'.repeat(64));
    fs.writeFileSync(platformKeyPath(), raw);
    keychain('denied');
    await osCryptReady();

    const manager = new DesktopAuthManager();
    expect(() => manager.getPlatformEncryptionEnvironment()).toThrow(KeychainUnavailableError);
    expect(fs.readFileSync(platformKeyPath()).equals(raw)).toBe(true);
  });

  it('secure-store throws instead of answering "no secret", and never overwrites a sealed file', async () => {
    const file = path.join(env.userData, 'secrets', 'fabric-worker-token.bin');
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const raw = await sealed('worker-token');
    fs.writeFileSync(file, raw);
    keychain('denied');
    await osCryptReady();

    expect(() => readSecret('fabric-worker-token')).toThrow(KeychainUnavailableError);
    expect(() => writeSecret('fabric-worker-token', 'replacement')).toThrow(KeychainUnavailableError);
    expect(fs.readFileSync(file).equals(raw)).toBe(true);

    keychain('ok');
    await osCryptReady();
    expect(readSecret('fabric-worker-token')).toBe('worker-token');
  });
});
