import { beforeEach, describe, expect, it } from 'vitest';
import {
  __setOsCryptTestSeam,
  decryptString,
  encryptString,
  KeychainUnavailableError,
  osCryptReady,
  osCryptState,
  type KeychainAddon,
} from './os-crypt.js';

// pbkdf2-hmac-sha1('testpassword','saltysalt',1003,16) from python hashlib;
// ciphertext from `openssl enc -aes-128-cbc -K <key> -iv 2020...20`.
const VECTOR = Buffer.concat([Buffer.from('v10'), Buffer.from('3dde8ae743aaf603aad44d3138ad2d60', 'hex')]);

function fakeAddon(initial: string | null) {
  let stored = initial;
  const calls: string[] = [];
  const addon: KeychainAddon = {
    async getPassword() {
      calls.push('get');
      return stored === null ? { status: 'not_found' } : { status: 'ok', password: Buffer.from(stored) };
    },
    async addPassword(_s, _a, pw) {
      calls.push('add');
      stored = pw.toString();
      return { status: 'ok' };
    },
  };
  return { addon, calls, stored: () => stored };
}

function seam(addon: KeychainAddon) {
  __setOsCryptTestSeam({ platform: 'darwin', appName: 'Test', loadAddon: () => addon });
}

describe('os-crypt (darwin codec)', () => {
  beforeEach(() => __setOsCryptTestSeam(null));

  it('throws KeychainUnavailableError(pending) before unlock', () => {
    seam(fakeAddon('testpassword').addon);
    expect(() => encryptString('x')).toThrow(KeychainUnavailableError);
    try {
      decryptString(VECTOR);
    } catch (e) {
      expect((e as KeychainUnavailableError).reason).toBe('pending');
    }
  });

  it('matches the independently computed known-answer vector', async () => {
    seam(fakeAddon('testpassword').addon);
    expect(await osCryptReady()).toBe('ready');
    expect(encryptString('hello world').equals(VECTOR)).toBe(true);
    expect(decryptString(VECTOR)).toBe('hello world');
  });

  it('round-trips unicode and rejects non-v10 input', async () => {
    seam(fakeAddon('testpassword').addon);
    await osCryptReady();
    expect(decryptString(encryptString('päss ✓'))).toBe('päss ✓');
    expect(() => decryptString(Buffer.from('v11abcdefghijklmnop'))).toThrow(/v10/);
  });

  it('creates the item when not found, then reads it back', async () => {
    const f = fakeAddon(null);
    seam(f.addon);
    expect(await osCryptReady()).toBe('ready');
    expect(f.calls).toEqual(['get', 'add', 'get']);
    expect(Buffer.from(f.stored()!, 'base64')).toHaveLength(16);
    expect(decryptString(encryptString('a'))).toBe('a');
  });

  it('reports denied and is retryable', async () => {
    let deny = true;
    const ok = fakeAddon('testpassword').addon;
    seam({
      getPassword: async (s, a) => (deny ? { status: 'denied', code: -128 } : ok.getPassword(s, a)),
      addPassword: ok.addPassword,
    });
    expect(await osCryptReady()).toBe('unavailable');
    expect(osCryptState()).toBe('denied');
    expect(() => encryptString('x')).toThrow(/denied/);
    deny = false;
    expect(await osCryptReady()).toBe('ready');
  });
});
