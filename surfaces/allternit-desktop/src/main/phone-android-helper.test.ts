import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { sha256Hex } from './phone-adb.js';
import { ANDROID_HELPER_VERSION, AndroidHelperUnavailableError, ensureAndroidHelper, parseAndroidHelperManifest, PINNED_ANDROID_MANIFESTS } from './phone-android-helper.js';

const dirs: string[] = [];
function scratch(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'android-helper-test-'));
  dirs.push(dir);
  return dir;
}
afterEach(() => {
  for (const dir of dirs.splice(0)) fs.rmSync(dir, { recursive: true, force: true });
  delete process.env.ALLTERNIT_PHONE_LINK_ANDROID_APK;
});

const MANIFEST = { version: '1.0.0', versionCode: 7, apk: { file: 'phonelink.apk', sha256: sha256Hex(Buffer.from('apk-bytes')), size: 9 } };
const BASE = 'https://runtime.example.com/phone-link/android';

/** Serves files from a map; records every requested URL. */
function fakeFetch(files: Record<string, Buffer>) {
  const requested: string[] = [];
  const fetchImpl = (async (input: string | URL) => {
    const url = String(input);
    requested.push(url);
    const body = files[url.replace(`${BASE}/`, '')];
    if (!body) return { ok: false, status: 404, arrayBuffer: async () => new ArrayBuffer(0) } as Response;
    return { ok: true, status: 200, arrayBuffer: async () => body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength) } as Response;
  }) as typeof fetch;
  return { fetchImpl, requested };
}

describe('parseAndroidHelperManifest', () => {
  it('accepts a well-formed manifest and rejects the rest', () => {
    expect(parseAndroidHelperManifest(JSON.stringify(MANIFEST))).toEqual(MANIFEST);
    for (const bad of [
      { ...MANIFEST, versionCode: '7' },
      { ...MANIFEST, versionCode: 0 },
      { ...MANIFEST, apk: { ...MANIFEST.apk, file: 'evil.exe' } },
      { ...MANIFEST, apk: { ...MANIFEST.apk, sha256: 'zz' } },
      { ...MANIFEST, apk: { ...MANIFEST.apk, size: -1 } },
    ]) {
      expect(() => parseAndroidHelperManifest(JSON.stringify(bad))).toThrow(AndroidHelperUnavailableError);
    }
  });
});

describe('ensureAndroidHelper', () => {
  it('throws for a version with no pinned manifest', async () => {
    await expect(ensureAndroidHelper(scratch(), { version: '0.0.0-unpinned' })).rejects.toMatchObject({ code: 'helper_unavailable' });
  });

  it('pins the published 0.3.1 helper', () => {
    expect(ANDROID_HELPER_VERSION).toBe('0.3.1');
    expect(PINNED_ANDROID_MANIFESTS['0.3.1']).toBe('32db339799c16054015831cf307ee205ce5195aba28b09c165eb7b4f4443014c');
  });

  it('downloads, verifies and caches the manifest and APK', async () => {
    const dataDir = scratch();
    const manifestBytes = Buffer.from(JSON.stringify(MANIFEST));
    const { fetchImpl, requested } = fakeFetch({
      '1.0.0/manifest.json': manifestBytes,
      '1.0.0/phonelink.apk': Buffer.from('apk-bytes'),
    });
    const deps = { baseUrl: BASE, version: '1.0.0', manifestSha256: sha256Hex(manifestBytes), fetchImpl };
    const first = await ensureAndroidHelper(dataDir, deps);
    expect(first.manifest).toEqual(MANIFEST);
    expect(first.apk).toBe(path.join(dataDir, 'android-helper', '1.0.0', 'phonelink.apk'));
    expect(requested).toEqual([`${BASE}/1.0.0/manifest.json`, `${BASE}/1.0.0/phonelink.apk`]);

    // Second run: everything verifies from the cache, no new downloads.
    const second = await ensureAndroidHelper(dataDir, deps);
    expect(second.apk).toBe(first.apk);
    expect(requested).toHaveLength(2);
  });

  it('refuses a manifest or APK that does not match the pin', async () => {
    const goodManifest = Buffer.from(JSON.stringify(MANIFEST));
    const badManifest = Buffer.from(JSON.stringify({ ...MANIFEST, versionCode: 8 }));
    const badApk = Buffer.from('tampered-apk-bytes');
    const deps = { baseUrl: BASE, version: '1.0.0', manifestSha256: sha256Hex(goodManifest) };

    const wrongPin = fakeFetch({ '1.0.0/manifest.json': badManifest });
    await expect(ensureAndroidHelper(scratch(), { ...deps, fetchImpl: wrongPin.fetchImpl })).rejects.toMatchObject({ code: 'helper_unavailable' });

    const badChecksum = fakeFetch({ '1.0.0/manifest.json': goodManifest, '1.0.0/phonelink.apk': badApk });
    await expect(ensureAndroidHelper(scratch(), { ...deps, fetchImpl: badChecksum.fetchImpl })).rejects.toMatchObject({ code: 'helper_unavailable' });
  });

  it('rejects a non-https base URL', async () => {
    const manifestBytes = Buffer.from(JSON.stringify(MANIFEST));
    await expect(
      ensureAndroidHelper(scratch(), {
        baseUrl: 'http://runtime.example.com/phone-link/android',
        version: '1.0.0',
        manifestSha256: sha256Hex(manifestBytes),
      }),
    ).rejects.toMatchObject({ code: 'helper_unavailable' });
  });

  it('uses ALLTERNIT_PHONE_LINK_ANDROID_APK as a dev override and still requires the file', async () => {
    const dataDir = scratch();
    const apk = path.join(dataDir, 'dev.apk');
    fs.writeFileSync(apk, 'dev');
    process.env.ALLTERNIT_PHONE_LINK_ANDROID_APK = apk;
    const helper = await ensureAndroidHelper(dataDir);
    expect(helper.apk).toBe(apk);
    expect(helper.manifest).toBeNull();

    process.env.ALLTERNIT_PHONE_LINK_ANDROID_APK = path.join(dataDir, 'missing.apk');
    await expect(ensureAndroidHelper(dataDir)).rejects.toMatchObject({ code: 'helper_unavailable' });
  });
});
