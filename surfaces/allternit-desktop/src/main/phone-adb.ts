/**
 * adb for the phone lane: resolve the binary, download Google's platform-tools
 * on first use, and run commands.
 *
 * Licensing: platform-tools ships under the Android SDK License Agreement,
 * which does not grant a right to redistribute the binaries inside our
 * installer. So we never bundle it: a user-installed adb is used when found,
 * otherwise the pinned official archive is downloaded from dl.google.com on
 * first use and verified against a pinned SHA-256 before it is unpacked. The
 * SHA-1 values in Google's repository2-3.xml manifest match these archives.
 */

import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import type { AdbCall, AdbResult } from './phone-tools.js';

export const PLATFORM_TOOLS_VERSION = 'r37.0.1';

export interface PlatformToolsArchive {
  url: string;
  sha256: string;
  size: number;
}

const BASE = 'https://dl.google.com/android/repository';

export const PLATFORM_TOOLS: Record<'darwin' | 'linux' | 'win32', PlatformToolsArchive> = {
  darwin: {
    url: `${BASE}/platform-tools_${PLATFORM_TOOLS_VERSION}-darwin.zip`,
    sha256: 'ee39ad5967e95c2a07f04dbcbde96b1a0c916ba376096db5d2f498b7727a5d1d',
    size: 16_110_554,
  },
  linux: {
    url: `${BASE}/platform-tools_${PLATFORM_TOOLS_VERSION}-linux.zip`,
    sha256: 'd230f13842f60f782a8645f9c813f8f845bf36089ea7289f28c48f17979313f1',
    size: 9_054_187,
  },
  win32: {
    url: `${BASE}/platform-tools_${PLATFORM_TOOLS_VERSION}-win.zip`,
    sha256: '45f4d63113e895ebde0c90f194099a4676b6ac653bd28d54314a9e022bbc1a99',
    size: 8_044_989,
  },
};

export function adbBinaryName(platform: NodeJS.Platform = process.platform): string {
  return platform === 'win32' ? 'adb.exe' : 'adb';
}

export function sha256Hex(data: Buffer): string {
  return createHash('sha256').update(data).digest('hex');
}

export function verifyArchive(data: Buffer, archive: PlatformToolsArchive): boolean {
  return data.length === archive.size && sha256Hex(data) === archive.sha256;
}

/** Where a previously downloaded platform-tools lives. */
export function downloadedAdbPath(dataDir: string, platform: NodeJS.Platform = process.platform): string {
  return path.join(dataDir, 'platform-tools', PLATFORM_TOOLS_VERSION, 'platform-tools', adbBinaryName(platform));
}

/** User-installed adb, in order: env override, downloaded copy, common install paths. */
export function resolveAdb(dataDir: string, platform: NodeJS.Platform = process.platform, includeSystem = true): string | null {
  const name = adbBinaryName(platform);
  const candidates = [
    process.env.ALLTERNIT_ADB_PATH,
    downloadedAdbPath(dataDir, platform),
    ...(includeSystem
      ? [
          ...(process.env.PATH ?? '').split(path.delimiter).filter(Boolean).map((dir) => path.join(dir, name)),
          platform === 'darwin' ? '/opt/homebrew/bin/adb' : undefined,
          platform === 'darwin' ? '/usr/local/bin/adb' : undefined,
          path.join(os.homedir(), 'Library', 'Android', 'sdk', 'platform-tools', name),
          path.join(os.homedir(), 'Android', 'Sdk', 'platform-tools', name),
        ]
      : []),
  ];
  for (const candidate of candidates) {
    if (candidate && fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function archiveKey(platform: NodeJS.Platform): keyof typeof PLATFORM_TOOLS | null {
  return platform === 'darwin' || platform === 'linux' || platform === 'win32' ? platform : null;
}

function run(command: string, args: string[]): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(command, args, { windowsHide: true }, (error, _stdout, stderr) => {
      if (error) reject(new Error(`${command} failed: ${stderr || error.message}`));
      else resolve();
    });
  });
}

export interface EnsureAdbDeps {
  fetchImpl?: typeof fetch;
  extract?: (zipPath: string, destDir: string) => Promise<void>;
  platform?: NodeJS.Platform;
  /** Tests set false so a developer's own adb doesn't mask the download path. */
  includeSystem?: boolean;
}

/** Return a usable adb path, downloading and verifying platform-tools if none exists. */
export async function ensureAdb(dataDir: string, deps: EnsureAdbDeps = {}): Promise<string> {
  const platform = deps.platform ?? process.platform;
  const existing = resolveAdb(dataDir, platform, deps.includeSystem ?? true);
  if (existing) return existing;

  const key = archiveKey(platform);
  if (!key) throw new Error(`No adb download for ${platform}; install platform-tools and set ALLTERNIT_ADB_PATH.`);
  const archive = PLATFORM_TOOLS[key];
  const response = await (deps.fetchImpl ?? fetch)(archive.url);
  if (!response.ok) throw new Error(`Downloading platform-tools failed: HTTP ${response.status}`);
  const data = Buffer.from(await response.arrayBuffer());
  if (!verifyArchive(data, archive)) {
    throw new Error('Downloaded platform-tools did not match the pinned checksum; refusing to install it.');
  }

  const destDir = path.join(dataDir, 'platform-tools', PLATFORM_TOOLS_VERSION);
  fs.mkdirSync(destDir, { recursive: true });
  const zipPath = path.join(destDir, 'platform-tools.zip');
  fs.writeFileSync(zipPath, data);
  const extract =
    deps.extract ??
    ((zip, dest) => (platform === 'win32' ? run('tar', ['-xf', zip, '-C', dest]) : run('unzip', ['-q', '-o', zip, '-d', dest])));
  await extract(zipPath, destDir);
  fs.rmSync(zipPath, { force: true });

  const adb = downloadedAdbPath(dataDir, platform);
  if (!fs.existsSync(adb)) throw new Error('platform-tools unpacked but adb was not found.');
  if (platform !== 'win32') fs.chmodSync(adb, 0o755);
  return adb;
}

/** The real runner: `adb [-s serial] args…`, no shell, bounded by a timeout. */
export function createAdbCall(adbPath: string): AdbCall {
  const build = (args: string[], serial?: string) => (serial ? ['-s', serial, ...args] : args);
  const call = ((args, opts = {}) =>
    new Promise<AdbResult>((resolve) => {
      execFile(
        adbPath,
        build(args, opts.serial),
        { timeout: opts.timeoutMs ?? 15_000, windowsHide: true, maxBuffer: 16 * 1024 * 1024 },
        (error, stdout, stderr) => {
          const code = error ? (typeof (error as NodeJS.ErrnoException).code === 'number' ? ((error as { code: number }).code) : 1) : 0;
          resolve({ stdout: String(stdout), stderr: String(stderr), code });
        },
      );
    })) as AdbCall;
  call.binary = (args, opts = {}) =>
    new Promise<Buffer>((resolve, reject) => {
      execFile(
        adbPath,
        build(args, opts.serial),
        { timeout: opts.timeoutMs ?? 15_000, windowsHide: true, maxBuffer: 64 * 1024 * 1024, encoding: 'buffer' },
        (error, stdout) => (error ? reject(error) : resolve(stdout)),
      );
    });
  return call;
}
