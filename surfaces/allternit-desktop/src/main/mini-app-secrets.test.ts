import { afterEach, beforeAll, describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { app } from 'electron';
import { __setOsCryptTestSeam, osCryptReady } from './os-crypt.js';
import {
  deleteAllMiniAppSecrets,
  deleteMiniAppSecret,
  getMiniAppSecretEnvironment,
  listMiniAppSecrets,
  setMiniAppSecret,
} from './mini-app-secrets.js';

const secretsFile = () => path.join(app.getPath('userData'), 'mini-app-secrets.json');

// Real os-crypt codec (macOS mode) over an in-memory Keychain item.
beforeAll(async () => {
  __setOsCryptTestSeam({
    platform: 'darwin',
    appName: 'Test',
    loadAddon: () => ({
      getPassword: async () => ({ status: 'ok', password: Buffer.from('test-keychain-password') }),
      addPassword: async () => ({ status: 'ok' }),
    }),
  });
  expect(await osCryptReady()).toBe('ready');
});

afterEach(() => {
  try { fs.unlinkSync(secretsFile()); } catch { /* absent */ }
});

describe('mini-app secrets: storage invariants', () => {
  it('stores values encrypted at rest with owner-only permissions', () => {
    expect(setMiniAppSecret('app.demo', 'API_TOKEN', 'supersecretvalue').success).toBe(true);
    const raw = fs.readFileSync(secretsFile(), 'utf8');
    expect(raw.includes('supersecretvalue')).toBe(false);
    expect((fs.statSync(secretsFile()).mode & 0o777) === 0o600).toBe(true);
  });

  it('rejects invalid app ids and secret names', () => {
    expect(setMiniAppSecret('bad id!', 'API_TOKEN', 'x').success).toBe(false);
    expect(setMiniAppSecret('app.demo', 'lowercase', 'x').success).toBe(false);
    expect(setMiniAppSecret('app.demo', 'A', 'x').success).toBe(false);
    expect(setMiniAppSecret('app.demo', 'VALID_NAME_1', 'x').success).toBe(true);
  });

  it('isolates secrets per miniapp', () => {
    setMiniAppSecret('app.one', 'TOKEN', 'one-value');
    setMiniAppSecret('app.two', 'TOKEN', 'two-value');
    expect(getMiniAppSecretEnvironment('app.one', ['TOKEN']).TOKEN).toBe('one-value');
    expect(getMiniAppSecretEnvironment('app.two', ['TOKEN']).TOKEN).toBe('two-value');
  });
});

describe('mini-app secrets: leak guards', () => {
  it('list returns names only, never values', () => {
    setMiniAppSecret('app.demo', 'API_TOKEN', 'supersecretvalue');
    const names = listMiniAppSecrets('app.demo');
    expect(names).toEqual(['API_TOKEN']);
    expect(names.join('').includes('supersecretvalue')).toBe(false);
  });

  it('injects only explicitly requested names into the environment', () => {
    setMiniAppSecret('app.demo', 'GRANTED', 'yes');
    setMiniAppSecret('app.demo', 'WITHHELD', 'no');
    const environment = getMiniAppSecretEnvironment('app.demo', ['GRANTED']);
    expect(environment.GRANTED).toBe('yes');
    expect('WITHHELD' in environment).toBe(false);
  });

  it('ignores requests for secrets that were never set', () => {
    const environment = getMiniAppSecretEnvironment('app.demo', ['MISSING']);
    expect(Object.keys(environment)).toHaveLength(0);
  });

  it('ignores damaged stored values instead of throwing', () => {
    setMiniAppSecret('app.demo', 'GOOD', 'fine');
    const data = JSON.parse(fs.readFileSync(secretsFile(), 'utf8'));
    data['app.demo'].BROKEN = '!!!not-base64!!!';
    fs.writeFileSync(secretsFile(), JSON.stringify(data), { mode: 0o600 });
    const environment = getMiniAppSecretEnvironment('app.demo', ['GOOD', 'BROKEN']);
    expect(environment.GOOD).toBe('fine');
    expect('BROKEN' in environment).toBe(false);
  });

  it('delete removes a single secret and deleteAll removes the app entry', () => {
    setMiniAppSecret('app.demo', 'ONE', '1');
    setMiniAppSecret('app.demo', 'TWO', '2');
    deleteMiniAppSecret('app.demo', 'ONE');
    expect(listMiniAppSecrets('app.demo')).toEqual(['TWO']);
    deleteAllMiniAppSecrets('app.demo');
    expect(listMiniAppSecrets('app.demo')).toHaveLength(0);
    const raw = fs.existsSync(secretsFile()) ? fs.readFileSync(secretsFile(), 'utf8') : '{}';
    expect(raw.includes('app.demo')).toBe(false);
  });
});
