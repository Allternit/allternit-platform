import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { RuntimePackages, type RuntimeManifest } from './runtime-package.js';

const { privateKey, publicKey } = crypto.generateKeyPairSync('ed25519');
const PUB = publicKey.export({ type: 'spki', format: 'pem' }).toString();
const sign = (data: Buffer | string) => crypto.sign(null, Buffer.from(data), privateKey).toString('base64');
const sha = (b: Buffer | string) => crypto.createHash('sha256').update(b).digest('hex');

let tmp: string;
let resources: string;
let root: string;

beforeEach(() => {
  tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'rtpkg-'));
  resources = path.join(tmp, 'resources');
  root = path.join(tmp, 'userData', 'runtime');
  fs.mkdirSync(path.join(resources, 'bin'), { recursive: true });
  fs.writeFileSync(path.join(resources, 'bin', 'allternit-api'), 'bundled');
  fs.writeFileSync(path.join(resources, 'runtime.json'), JSON.stringify({ build: 100 }));
});
afterEach(() => fs.rmSync(tmp, { recursive: true, force: true }));

function makePackage(version: string, build: number, opts: { tamper?: boolean; shellApi?: number } = {}) {
  const dir = path.join(tmp, 'src', version);
  fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
  const body = `api ${version}`;
  fs.writeFileSync(path.join(dir, 'bin', 'allternit-api'), body);
  const manifest: RuntimeManifest = {
    version, build, platform: 'test-x', shellApi: opts.shellApi ?? 1,
    files: { 'bin/allternit-api': { sha256: sha(body), size: body.length } },
  };
  const raw = JSON.stringify(manifest);
  fs.writeFileSync(path.join(dir, 'manifest.json'), raw);
  fs.writeFileSync(path.join(dir, 'manifest.sig'), sign(raw));
  if (opts.tamper) fs.writeFileSync(path.join(dir, 'bin', 'allternit-api'), `API ${version}`);
  const archive = path.join(tmp, `${version}.tar.gz`);
  execFileSync('tar', ['-czf', archive, '-C', dir, '.']);
  return archive;
}

function feed(version: string, build: number, archive: string, shellApi = 1): typeof fetch {
  const bytes = fs.readFileSync(archive);
  const latest = JSON.stringify({ version, build, shellApi, url: `${version}.tar.gz`, sha256: sha(bytes), size: bytes.length });
  return (async (url: string | URL) => {
    const u = String(url);
    if (u.endsWith('/latest.json')) return new Response(latest);
    if (u.endsWith('/latest.json.sig')) return new Response(sign(latest));
    if (u.endsWith(`${version}.tar.gz`)) return new Response(bytes);
    return new Response('nope', { status: 404 });
  }) as typeof fetch;
}

const store = () => new RuntimePackages({ root, resourcesPath: resources, publicKey: PUB, platform: 'test-x', shellApi: 1 });

describe('RuntimePackages', () => {
  it('uses the bundled runtime when nothing is installed', async () => {
    const s = store();
    expect(await s.prepareForLaunch()).toBeNull();
    expect(fs.readFileSync(s.file('bin', 'allternit-api'), 'utf8')).toBe('bundled');
  });

  it('stages a signed newer package and runs it after restart', async () => {
    const s = store();
    expect(await s.checkForUpdate('https://feed.test', feed('r2', 200, makePackage('r2', 200)))).toBe('r2');
    expect(s.hasPendingUpdate()).toBe(true);
    const next = store();
    await next.prepareForLaunch();
    expect(fs.readFileSync(next.file('bin', 'allternit-api'), 'utf8')).toBe('api r2');
    expect(next.readState().trial).toBe('r2');
    next.confirmHealthy();
    expect(next.readState().active).toBe('r2');
    expect(next.readState().trial).toBeUndefined();
  });

  it('ignores packages no newer than the bundled runtime', async () => {
    expect(await store().checkForUpdate('https://feed.test', feed('r1', 100, makePackage('r1', 100)))).toBeNull();
  });

  it('refuses a package whose files do not match its signed manifest', async () => {
    await expect(store().checkForUpdate('https://feed.test', feed('r3', 300, makePackage('r3', 300, { tamper: true }))))
      .rejects.toThrow(/verification/);
    expect(fs.existsSync(path.join(root, 'versions', 'r3'))).toBe(false);
  });

  it('refuses a feed signed by another key', async () => {
    const other = crypto.generateKeyPairSync('ed25519').publicKey.export({ type: 'spki', format: 'pem' }).toString();
    const s = new RuntimePackages({ root, resourcesPath: resources, publicKey: other, platform: 'test-x', shellApi: 1 });
    await expect(s.checkForUpdate('https://feed.test', feed('r2', 200, makePackage('r2', 200)))).rejects.toThrow(/signature/);
  });

  it('skips packages that need a newer shell', async () => {
    expect(await store().checkForUpdate('https://feed.test', feed('r4', 400, makePackage('r4', 400, { shellApi: 2 }), 2))).toBeNull();
  });

  it('rolls back a trial that failed to start and never retries it', async () => {
    await store().checkForUpdate('https://feed.test', feed('r2', 200, makePackage('r2', 200)));
    const s = store();
    await s.prepareForLaunch();
    expect(s.failTrial()).toBe(true);
    const after = store();
    expect(await after.prepareForLaunch()).toBeNull();
    expect(after.readState().bad).toContain('r2');
    expect(await after.checkForUpdate('https://feed.test', feed('r2', 200, makePackage('r2', 200)))).toBeNull();
  });

  it('rolls back a trial that crashed before confirming', async () => {
    await store().checkForUpdate('https://feed.test', feed('r2', 200, makePackage('r2', 200)));
    await store().prepareForLaunch(); // trial, never confirmed
    const s = store();
    expect(await s.prepareForLaunch()).toBeNull();
    expect(s.readState().bad).toContain('r2');
  });

  it('drops a downloaded runtime once the app bundles a newer one', async () => {
    await store().checkForUpdate('https://feed.test', feed('r2', 200, makePackage('r2', 200)));
    const s = store();
    await s.prepareForLaunch();
    s.confirmHealthy();
    fs.writeFileSync(path.join(resources, 'runtime.json'), JSON.stringify({ build: 500 }));
    const updatedApp = store();
    expect(await updatedApp.prepareForLaunch()).toBeNull();
    expect(fs.existsSync(path.join(root, 'versions', 'r2'))).toBe(false);
  });
});
