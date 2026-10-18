/**
 * The Phone Link Android app Desktop installs on the user's phone: download +
 * verify the APK we publish (com.allternit.phonelink, the iCTRL protocol v1
 * server that starts on boot; source in surfaces/allternit-mobile/android).
 *
 * Same pinned-download pattern as the iPhone runner (phone-ios-runner.ts):
 * the orchestrator publishes <base>/<version>/manifest.json + the APK; the
 * manifest's own SHA-256 is pinned here per version, so a configurable base
 * URL can't swap in another build. Until a version is pinned, Android setup
 * ends with helper_unavailable rather than installing something unverified.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { sha256Hex } from './phone-adb.js';
import type { FetchFn } from './ictrl-client.js';

export const DEFAULT_ANDROID_HELPER_BASE_URL = 'https://runtime.allternit.com/phone-link/android';

/**
 * version → SHA-256 of its manifest.json. Empty until the first APK is
 * published; the packaging step prints the line to add here.
 */
export const PINNED_ANDROID_MANIFESTS: Record<string, string> = {
  '0.1.0': '85436afac9016a3a1707c9f42e4348482a66fdb93da744d460bd15f4c2ffa7ec',
  // 0.2.1: joins the mesh (own :mesh process), 16 MB. (0.2.0 was never pinned:
  // a failed upload left a cached 404 on its APK URL for 4 h.)
  '0.2.1': 'b177cbe831009d7ea907b7a86456cfe23511e2fe413eb3777553221cc2b44287',
  // 0.3.1: shell server + in-app adb (screen over Wi-Fi), carrier stubs skipped.
  '0.3.1': '32db339799c16054015831cf307ee205ce5195aba28b09c165eb7b4f4443014c',
};
export const ANDROID_HELPER_VERSION: string | null = '0.3.1';

export interface AndroidHelperManifest {
  version: string;
  versionCode: number;
  apk: { file: string; sha256: string; size: number };
}

/** The resolved helper: the APK path and, unless a dev override supplied the APK, its manifest. */
export interface AndroidHelper {
  apk: string;
  manifest: AndroidHelperManifest | null;
}

export class AndroidHelperUnavailableError extends Error {
  readonly code = 'helper_unavailable';
}

export function parseAndroidHelperManifest(text: string): AndroidHelperManifest {
  const m = JSON.parse(text) as AndroidHelperManifest;
  if (
    typeof m.version !== 'string' ||
    !/^[\w.-]+$/.test(m.version) ||
    !Number.isInteger(m.versionCode) ||
    m.versionCode <= 0 ||
    typeof m.apk?.file !== 'string' ||
    !/^[\w.-]+\.apk$/.test(m.apk.file) ||
    !/^[0-9a-f]{64}$/.test(m.apk?.sha256 ?? '') ||
    !Number.isInteger(m.apk?.size) ||
    (m.apk?.size ?? 0) <= 0
  ) {
    throw new AndroidHelperUnavailableError('The Android helper manifest is malformed.');
  }
  return m;
}

export interface EnsureAndroidHelperDeps {
  baseUrl?: string;
  version?: string | null;
  /** Overrides the pin (dev builds). */
  manifestSha256?: string;
  fetchImpl?: FetchFn;
}

/**
 * Download (once) and verify the helper APK. `ALLTERNIT_PHONE_LINK_ANDROID_APK`
 * points at a local APK for dev builds: no download, but the file must exist.
 */
export async function ensureAndroidHelper(dataDir: string, deps: EnsureAndroidHelperDeps = {}): Promise<AndroidHelper> {
  const override = process.env.ALLTERNIT_PHONE_LINK_ANDROID_APK;
  if (override) {
    if (!fs.existsSync(override)) throw new AndroidHelperUnavailableError(`ALLTERNIT_PHONE_LINK_ANDROID_APK points at ${override}, which does not exist.`);
    return { apk: override, manifest: null };
  }

  const version = deps.version ?? process.env.ALLTERNIT_PHONE_LINK_ANDROID_VERSION ?? ANDROID_HELPER_VERSION;
  const pin = deps.manifestSha256 ?? process.env.ALLTERNIT_PHONE_LINK_ANDROID_MANIFEST_SHA256 ?? (version ? PINNED_ANDROID_MANIFESTS[version] : undefined);
  if (!version || !pin) throw new AndroidHelperUnavailableError('The Android helper for this version of Allternit has not been published yet.');
  const base = (deps.baseUrl ?? process.env.ALLTERNIT_PHONE_LINK_ANDROID_BASE_URL ?? DEFAULT_ANDROID_HELPER_BASE_URL).replace(/\/+$/, '');
  if (!/^https:\/\//.test(base) && !/^http:\/\/127\.0\.0\.1[:/]/.test(base)) {
    throw new AndroidHelperUnavailableError('The helper download URL must be https.');
  }
  const fetchImpl = deps.fetchImpl ?? fetch;
  const dir = path.join(dataDir, 'android-helper', version);
  fs.mkdirSync(dir, { recursive: true });

  const get = async (name: string): Promise<Buffer> => {
    const res = await fetchImpl(`${base}/${version}/${name}`);
    if (!res.ok) throw new AndroidHelperUnavailableError(`Downloading ${name} failed: HTTP ${res.status}`);
    return Buffer.from(await res.arrayBuffer());
  };

  const manifestPath = path.join(dir, 'manifest.json');
  const cached = fs.existsSync(manifestPath) ? fs.readFileSync(manifestPath) : null;
  let manifestBytes: Buffer;
  if (cached && sha256Hex(cached) === pin) {
    manifestBytes = cached;
  } else {
    manifestBytes = await get('manifest.json');
    if (sha256Hex(manifestBytes) !== pin) {
      throw new AndroidHelperUnavailableError('The helper manifest did not match the pinned checksum; refusing to use it.');
    }
    fs.writeFileSync(manifestPath, manifestBytes);
  }
  const manifest = parseAndroidHelperManifest(manifestBytes.toString('utf8'));
  if (manifest.version !== version) throw new AndroidHelperUnavailableError('The helper manifest is for another version.');

  const file = path.join(dir, manifest.apk.file);
  if (fs.existsSync(file)) {
    const data = fs.readFileSync(file);
    if (data.length === manifest.apk.size && sha256Hex(data) === manifest.apk.sha256) return { apk: file, manifest };
  }
  const apk = await get(manifest.apk.file);
  if (apk.length !== manifest.apk.size || sha256Hex(apk) !== manifest.apk.sha256) {
    throw new AndroidHelperUnavailableError(`${manifest.apk.file} did not match its checksum; refusing to install it.`);
  }
  fs.writeFileSync(file, apk);
  return { apk: file, manifest };
}
