import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { internalServiceLaunchEnvironment, loadOrCreateInternalServiceToken } from './internal-service-token.js';
import { KeychainUnavailableError } from './os-crypt.js';

// Regression (2026-11-13): Desktop never set ALLTERNIT_INTERNAL_SERVICE_TOKEN,
// so gizzi skipped its bundled allternit-connectors/allternit-tools MCP servers
// and hosted agents had no phone/mail tools (`curl :4096/mcp` returned {}).

const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0)) fs.rmSync(dir, { recursive: true, force: true });
});

function store() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'internal-token-'));
  dirs.push(dir);
  const quarantined: string[] = [];
  return {
    quarantined,
    filePath: path.join(dir, 'auth', 'internal-service-token'),
    // Reversible stand-in for safeStorage, so the file never holds plain text.
    encode: (value: string) => Buffer.from(`enc:${Buffer.from(value).toString('base64')}`),
    decode: (raw: Buffer) => {
      const text = raw.toString('utf8');
      if (!text.startsWith('enc:')) throw new Error('not encrypted');
      return Buffer.from(text.slice(4), 'base64').toString('utf8');
    },
    quarantine: (filePath: string) => {
      quarantined.push(filePath);
      fs.renameSync(filePath, `${filePath}.corrupt`);
    },
  };
}

describe('internal service token', () => {
  it('gives allternit-api and gizzi the same token, and gizzi the runtime owner', () => {
    const s = store();
    const token = loadOrCreateInternalServiceToken(s);
    const env = internalServiceLaunchEnvironment(token, 'platform:proj_e72da9f1b8e8f2081b44c32a');

    expect(token).toMatch(/^[a-f0-9]{64}$/);
    expect(env.api.ALLTERNIT_INTERNAL_SERVICE_TOKEN).toBe(token);
    expect(env.gizzi.ALLTERNIT_INTERNAL_SERVICE_TOKEN).toBe(env.api.ALLTERNIT_INTERNAL_SERVICE_TOKEN);
    expect(env.gizzi.ALLTERNIT_USER_ID).toBe('platform:proj_e72da9f1b8e8f2081b44c32a');
    expect(env.api.ALLTERNIT_USER_ID).toBeUndefined();
  });

  it('leaves the owner unset when the runtime is not paired yet', () => {
    expect(internalServiceLaunchEnvironment('a'.repeat(64), null).gizzi).toEqual({
      ALLTERNIT_INTERNAL_SERVICE_TOKEN: 'a'.repeat(64),
    });
  });

  it('reuses the persisted token across launches, encrypted at 0600', () => {
    const s = store();
    const first = loadOrCreateInternalServiceToken(s);
    const second = loadOrCreateInternalServiceToken(s);

    expect(second).toBe(first);
    expect(fs.readFileSync(s.filePath, 'utf8')).not.toContain(first);
    if (process.platform !== 'win32') {
      expect(fs.statSync(s.filePath).mode & 0o777).toBe(0o600);
    }
  });

  it('replaces an unreadable token file instead of failing launch', () => {
    const s = store();
    fs.mkdirSync(path.dirname(s.filePath), { recursive: true });
    fs.writeFileSync(s.filePath, 'garbage');

    const token = loadOrCreateInternalServiceToken(s);

    expect(token).toMatch(/^[a-f0-9]{64}$/);
    expect(s.quarantined).toEqual([s.filePath]);
    expect(loadOrCreateInternalServiceToken(s)).toBe(token);
  });
  it('keeps the token file when the macOS Keychain is locked, instead of replacing it', () => {
    const s = store();
    const token = loadOrCreateInternalServiceToken(s);
    const before = fs.readFileSync(s.filePath);
    const locked = { ...s, decode: () => { throw new KeychainUnavailableError('pending'); } };
    expect(() => loadOrCreateInternalServiceToken(locked)).toThrow(KeychainUnavailableError);
    expect(s.quarantined).toEqual([]);
    expect(fs.readFileSync(s.filePath).equals(before)).toBe(true);
    expect(loadOrCreateInternalServiceToken(s)).toBe(token);
  });
});
