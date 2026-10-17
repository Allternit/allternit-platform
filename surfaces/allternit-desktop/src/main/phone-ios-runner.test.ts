import { afterEach, describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { sha256Hex } from './phone-adb.js';
import {
  ensureRunnerArtifacts,
  profileCovers,
  SideloadSigner,
  SigningError,
  type RunnerArtifacts,
  type SidecarProcess,
} from './phone-ios-runner.js';

const tmp: string[] = [];
afterEach(() => {
  for (const d of tmp.splice(0)) fs.rmSync(d, { recursive: true, force: true });
});

function published() {
  const runner = Buffer.from('runner ipa bytes');
  const host = Buffer.from('host ipa bytes');
  const manifest = Buffer.from(
    JSON.stringify({
      version: '1.0.0',
      protocol: 1,
      bundlePrefix: 'com.allternit.phonelink',
      runner: { file: 'iCTRLUITests-Runner.ipa', sha256: sha256Hex(runner), size: runner.length, bundleId: 'com.allternit.phonelink.ictrl.uitests.xctrunner', xctestConfig: 'iCTRLUITests.xctest', testToRun: 'iCTRLUITests.iCTRLUITests/testStartServer' },
      host: { file: 'iCTRL.ipa', sha256: sha256Hex(host), size: host.length, bundleId: 'com.allternit.phonelink.ictrl' },
    }),
  );
  const files: Record<string, Buffer> = { 'manifest.json': manifest, 'iCTRLUITests-Runner.ipa': runner, 'iCTRL.ipa': host };
  const urls: string[] = [];
  const fetchImpl = (async (url: string) => {
    urls.push(url);
    const data = files[url.split('/').pop()!];
    return { ok: !!data, status: data ? 200 : 404, arrayBuffer: async () => data } as unknown as Response;
  }) as unknown as typeof fetch;
  return { files, fetchImpl, urls, manifestSha: sha256Hex(manifest) };
}

const dataDir = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'runner-'));
  tmp.push(d);
  return d;
};

describe('runner artifacts', () => {
  it('downloads, verifies against the pinned manifest, and reuses the cache', async () => {
    const pub = published();
    const dir = dataDir();
    const opts = { baseUrl: 'https://runtime.example/phone-link/ictrl/', version: '1.0.0', manifestSha256: pub.manifestSha, fetchImpl: pub.fetchImpl };
    const a = await ensureRunnerArtifacts(dir, opts);
    expect(pub.urls).toEqual([
      'https://runtime.example/phone-link/ictrl/1.0.0/manifest.json',
      'https://runtime.example/phone-link/ictrl/1.0.0/iCTRLUITests-Runner.ipa',
      'https://runtime.example/phone-link/ictrl/1.0.0/iCTRL.ipa',
    ]);
    expect(fs.readFileSync(a.runnerIpa).toString()).toBe('runner ipa bytes');
    expect(a.manifest.runner.bundleId).toBe('com.allternit.phonelink.ictrl.uitests.xctrunner');
    await ensureRunnerArtifacts(dir, opts);
    expect(pub.urls).toHaveLength(3);
  });

  it('refuses a manifest that does not match the pin', async () => {
    const pub = published();
    await expect(
      ensureRunnerArtifacts(dataDir(), { baseUrl: 'https://r.example', version: '1.0.0', manifestSha256: 'f'.repeat(64), fetchImpl: pub.fetchImpl }),
    ).rejects.toMatchObject({ code: 'runner_unavailable' });
  });

  it('refuses a package that does not match the manifest', async () => {
    const pub = published();
    pub.files['iCTRL.ipa'] = Buffer.from('tampered ipa!!');
    await expect(
      ensureRunnerArtifacts(dataDir(), { baseUrl: 'https://r.example', version: '1.0.0', manifestSha256: pub.manifestSha, fetchImpl: pub.fetchImpl }),
    ).rejects.toThrow(/checksum/);
  });

  it('is unavailable until a version is pinned, and only over https', async () => {
    const prev = { v: process.env.ALLTERNIT_ICTRL_VERSION, s: process.env.ALLTERNIT_ICTRL_MANIFEST_SHA256 };
    delete process.env.ALLTERNIT_ICTRL_VERSION;
    delete process.env.ALLTERNIT_ICTRL_MANIFEST_SHA256;
    try {
      await expect(ensureRunnerArtifacts(dataDir())).rejects.toMatchObject({ code: 'runner_unavailable' });
      await expect(ensureRunnerArtifacts(dataDir(), { baseUrl: 'http://evil.example', version: '1', manifestSha256: 'a'.repeat(64) })).rejects.toThrow(/https/);
    } finally {
      if (prev.v !== undefined) process.env.ALLTERNIT_ICTRL_VERSION = prev.v;
      if (prev.s !== undefined) process.env.ALLTERNIT_ICTRL_MANIFEST_SHA256 = prev.s;
    }
  });
});

describe('profileCovers', () => {
  it('scores exact, prefix and full wildcards', () => {
    expect(profileCovers('T.com.a.ictrl', 'T', 'com.a.ictrl')).toBe(2);
    expect(profileCovers('T.com.a.*', 'T', 'com.a.ictrl')).toBe(1);
    expect(profileCovers('T.*', 'T', 'com.a.ictrl')).toBe(0);
    expect(profileCovers('T.com.b.*', 'T', 'com.a.ictrl')).toBe(-1);
    expect(profileCovers('OTHER.*', 'T', 'com.a.ictrl')).toBe(-1);
  });
});

describe('SideloadSigner (allternit-sideload protocol)', () => {
  const artifacts = { runnerIpa: '/r.ipa', hostIpa: '/h.ipa', manifest: {} } as unknown as RunnerArtifacts;

  function fakeSidecar(script: (msg: Record<string, unknown>, emit: (o: object) => void) => void) {
    const written: Record<string, unknown>[] = [];
    let handler: (line: string) => void = () => {};
    let exit!: (code: number | null) => void;
    const proc: SidecarProcess = {
      write: (line) => {
        const msg = JSON.parse(line);
        written.push(msg);
        queueMicrotask(() => script(msg, (o) => handler(JSON.stringify(o))));
      },
      onLine: (h) => void (handler = h),
      exited: new Promise((r) => (exit = r)),
      kill: () => exit(0),
    };
    return { spawn: () => proc, written };
  }

  it('sends the password once on stdin, relays 2FA, and returns the signed packages', async () => {
    const fake = fakeSidecar((msg, emit) => {
      if (msg.op === 'sign') emit({ event: 'two_factor_required', sms: false });
      if (msg.op === 'two_factor')
        emit({ event: 'done', runner_ipa: '/o/r.ipa', host_ipa: '/o/h.ipa', bundle_prefix: 'com.allternit.phonelink', runner_bundle_id: 'r.T', host_bundle_id: 'h.T', team_id: 'T', expires_at: '2026-10-23T00:00:00Z' });
    });
    const dir = dataDir();
    const signer = new SideloadSigner({ binary: '/bin/allternit-sideload', spawn: fake.spawn });
    const result = await signer.sign({
      udid: 'U',
      deviceName: 'iPhone',
      artifacts,
      workDir: dir,
      credentials: { appleId: 'a@b.c', password: 'hunter2' },
      twoFactor: async () => '654321',
    });
    expect(result).toMatchObject({ runnerBundleId: 'r.T', teamId: 'T', expiresAt: '2026-10-23T00:00:00Z' });
    expect(fake.written[0]).toMatchObject({ op: 'sign', apple_id: 'a@b.c', password: 'hunter2', udid: 'U' });
    expect(fake.written[1]).toEqual({ op: 'two_factor', code: '654321' });
  });

  it('maps sidecar errors to typed SigningErrors', async () => {
    const fake = fakeSidecar((_m, emit) => emit({ event: 'error', code: 'bad_credentials', message: 'Incorrect password' }));
    const signer = new SideloadSigner({ binary: '/bin/x', spawn: fake.spawn });
    const error = await signer.sign({ udid: 'U', deviceName: 'i', artifacts, workDir: dataDir(), credentials: { appleId: 'a', password: 'b' } }).catch((e) => e);
    expect(error).toBeInstanceOf(SigningError);
    expect(error.code).toBe('bad_credentials');
  });

  it('is unavailable when the build has no sidecar', async () => {
    await expect(new SideloadSigner({ binary: null }).sign({ udid: 'U', deviceName: 'i', artifacts, workDir: '/x' })).rejects.toMatchObject({ code: 'signer_unavailable' });
  });
});
