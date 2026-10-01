import { mkdtempSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  bootstrapPath,
  consumeProvisionedBootstrap,
  isProvisionedMode,
  provisionedPairingBody,
  readProvisionedBootstrap,
} from './provisioned-bootstrap.js';

function file(content: unknown): string {
  const dir = mkdtempSync(join(tmpdir(), 'alt-bootstrap-'));
  const path = join(dir, 'bootstrap.json');
  writeFileSync(path, typeof content === 'string' ? content : JSON.stringify(content));
  return path;
}

const valid = { api: 'https://api.allternit.com/', token: 'tok_1', instance_id: 'pi_1', user_id: 'user_1' };

describe('provisioned bootstrap', () => {
  it('reads a valid bootstrap and normalises the API url', () => {
    const path = file(valid);
    expect(readProvisionedBootstrap(path)).toEqual({ api: 'https://api.allternit.com', token: 'tok_1', instanceId: 'pi_1', userId: 'user_1', path });
  });

  it('ignores a missing, malformed or insecure bootstrap (falls back to normal sign-in)', () => {
    expect(readProvisionedBootstrap('/nonexistent/bootstrap.json')).toBeNull();
    expect(readProvisionedBootstrap(file('{not json'))).toBeNull();
    expect(readProvisionedBootstrap(file({ ...valid, token: '' }))).toBeNull();
    expect(readProvisionedBootstrap(file({ ...valid, api: 'http://api.allternit.com' }))).toBeNull();
  });

  it('pairs as the cloud computer with the token, keeping the device key', () => {
    const bootstrap = readProvisionedBootstrap(file(valid))!;
    const body = provisionedPairingBody({ name: 'host Desktop', runtimeType: 'desktop', publicKey: 'pk' }, bootstrap);
    expect(body).toMatchObject({ name: 'Allternit cloud computer', runtimeType: 'provisioned', bootstrapToken: 'tok_1', instanceId: 'pi_1', publicKey: 'pk' });
  });

  it('is consumed after use and stays in provisioned mode through the image flag', () => {
    const path = file(valid);
    const bootstrap = readProvisionedBootstrap(path)!;
    expect(isProvisionedMode({}, path)).toBe(true);
    consumeProvisionedBootstrap(bootstrap);
    expect(existsSync(path)).toBe(false);
    expect(isProvisionedMode({}, path)).toBe(false);
    expect(isProvisionedMode({ ALLTERNIT_PROVISIONED: '1' }, path)).toBe(true);
  });

  it('lets tests and odd images point at another file', () => {
    expect(bootstrapPath({ ALLTERNIT_BOOTSTRAP_FILE: '/tmp/x.json' })).toBe('/tmp/x.json');
    expect(bootstrapPath({})).toBe('/etc/allternit/bootstrap.json');
  });
});
