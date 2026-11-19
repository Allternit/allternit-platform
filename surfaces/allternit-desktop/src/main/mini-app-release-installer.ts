/**
 * Atomic, versioned mini-app release installer.
 *
 * Installs marketplace releases from the Allternit registry into versioned
 * directories — never globally:
 *
 *   <userData>/mini-app-releases/<hash(id)>/
 *     versions/<x.y.z>/        extracted release, one immutable dir per version
 *     current -> versions/…    atomic symlink switch
 *     state.json               current/previous versions and release records
 *
 * Install flow:
 *   1. Fetch the release descriptor from the registry (main process only).
 *   2. Verify the publisher Ed25519 signature of the manifest — the registry's
 *      verified status is never trusted without an independent signature check.
 *   3. Download the release archive into a quarantine directory, hashing the
 *      stream; reject on any size/checksum mismatch.
 *   4. Extract into a staging dir with path-traversal protection (entries are
 *      validated before extraction and the extracted tree is walked after:
 *      no absolute paths, no '..', no symlinks, only files and directories).
 *   5. Rename into an immutable versions/<x.y.z>/ directory.
 *   6. Install dependencies inside the version dir (npm ci --ignore-scripts,
 *      sandboxed, only when the manifest declares network access).
 *   7. Atomically switch `current`.
 *   8. Run health checks through the `current` link (secrets are injected only
 *      if already stored; otherwise start-based verification is skipped).
 *   9. On failure, switch back to the previous version and remove the failed
 *      one; on success, keep the previous version for manual rollback.
 *
 * Command-based installs (`mini-apps-manager.ts`) remain available for
 * explicitly approved developer/local runtimes; this module is the community
 * marketplace path.
 */

import { spawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { PassThrough, Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { createHash, createPublicKey, verify as cryptoVerify } from 'node:crypto';
import { app } from 'electron';
import log from 'electron-log';
import { getMiniAppSecretEnvironment, listMiniAppSecrets } from './mini-app-secrets.js';
import { sandboxCommand, type MiniAppSandboxPermissions } from './mini-app-sandbox.js';
import { startMiniAppPolicyProxy } from './mini-app-policy-proxy.js';
import { extractZipSafely } from './zip-extract.js';

// ─── Types ────────────────────────────────────────────────────────────────────

export interface ReleaseAssetInfo {
  id: number;
  kind: string;
  sha256: string;
  sizeBytes: number;
  mime: string;
  quarantined: boolean;
  downloadUrl?: string;
  expiresAt?: number;
}

export interface MiniAppReleaseManifest {
  id?: string;
  name?: string;
  permissions?: MiniAppSandboxPermissions & { secrets?: string[] };
  lifecycle?: {
    start?: { command: string; args?: string[] };
    health?: { kind: 'http' | 'command' | 'process'; url?: string; command?: string; args?: string[] };
  };
  compatibility?: { allternit?: string; platforms?: string[] };
  release?: { signature?: string; publisherKey?: string; changelog?: string };
}

export interface ReleaseDescriptor {
  id: string;
  publisher: string;
  version: string;
  manifest: MiniAppReleaseManifest;
  signature?: string | null;
  publisherKey?: string | null;
  submittedAt: number;
  assets: ReleaseAssetInfo[];
}

export type ReleaseInstallProgress = { id: string; line: string; type: 'stdout' | 'stderr' | 'info' };

export interface ReleaseInstallResult {
  success: boolean;
  error?: string;
  id: string;
  version?: string;
  previousVersion?: string;
  rolledBack?: boolean;
}

interface ReleaseRecord {
  installedAt: string;
  sha256: string;
  signature: string;
  publisherKey: string;
  healthy: boolean;
}

interface ReleaseInstallState {
  id?: string;
  currentVersion?: string;
  previousVersion?: string;
  releases: Record<string, ReleaseRecord>;
}

export interface ReleaseInstallInfo {
  id: string;
  currentVersion?: string;
  previousVersion?: string;
  healthy: boolean;
  releases: Record<string, ReleaseRecord>;
}

// ─── Constants ────────────────────────────────────────────────────────────────

const MAX_ARCHIVE_BYTES = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES = 100_000;
const MAX_EXTRACTED_BYTES = 2 * 1024 * 1024 * 1024;
const MAX_LISTING_BYTES = 32 * 1024 * 1024;
const MINIAPP_ID_PATTERN = /^[a-z0-9][a-z0-9:._/-]{1,199}$/i;
const VERSION_PATTERN = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/;
const INSTALL_MARKER = '.allternit-install';

// ─── Paths and state ──────────────────────────────────────────────────────────

function releasesRoot(): string {
  return path.join(app.getPath('userData'), 'mini-app-releases');
}

function appDirFor(id: string): string {
  return path.join(releasesRoot(), createHash('sha256').update(id).digest('hex').slice(0, 24));
}

function statePath(appDir: string): string {
  return path.join(appDir, 'state.json');
}

function loadState(appDir: string): ReleaseInstallState {
  try {
    return JSON.parse(fs.readFileSync(statePath(appDir), 'utf8')) as ReleaseInstallState;
  } catch {
    return { releases: {} };
  }
}

function saveState(appDir: string, state: ReleaseInstallState): void {
  const temporary = `${statePath(appDir)}.tmp`;
  fs.writeFileSync(temporary, JSON.stringify(state, null, 2), { mode: 0o600 });
  fs.renameSync(temporary, statePath(appDir));
}

// ─── Signature verification ───────────────────────────────────────────────────

/**
 * Canonical manifest form for signing. This MUST match
 * `mini-app-manifest.ts` in the ai.allternit.com surface exactly: recursively
 * sort object keys (localeCompare), drop `undefined`, keep array order.
 */
export function canonicalizeForSigning(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalizeForSigning);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .filter(([, item]) => item !== undefined)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => [key, canonicalizeForSigning(item)]),
  );
}

function decodeKeyMaterial(value: string): Buffer {
  return Buffer.from(value.replace(/-/g, '+').replace(/_/g, '/'), 'base64');
}

/**
 * Verify the publisher Ed25519 signature over the canonical manifest with the
 * `signature` field removed (matching the marketplace client verifier).
 */
export function verifyManifestSignature(manifest: unknown, signature: string, publisherKey: string): boolean {
  try {
    const source = manifest as Record<string, unknown>;
    const release = (source.release ?? {}) as Record<string, unknown>;
    const unsigned = { ...source, release: { ...release, signature: undefined } };
    const rawKey = decodeKeyMaterial(publisherKey);
    if (rawKey.length !== 32) return false;
    const key = createPublicKey({
      key: { kty: 'OKP', crv: 'Ed25519', x: rawKey.toString('base64url') },
      format: 'jwk',
    });
    const signatureBytes = decodeKeyMaterial(signature);
    if (signatureBytes.length !== 64) return false;
    const message = Buffer.from(JSON.stringify(canonicalizeForSigning(unsigned)), 'utf8');
    return cryptoVerify(null, message, key, signatureBytes);
  } catch {
    return false;
  }
}

// ─── Registry client ──────────────────────────────────────────────────────────

function registryEndpoint(registryUrl: string, id: string, suffix: string): URL {
  const url = new URL(`${registryUrl.replace(/\/+$/, '')}/v1/miniapps/${encodeURIComponent(id)}${suffix}`);
  if (url.protocol !== 'https:' && !['localhost', '127.0.0.1', '::1'].includes(url.hostname)) {
    throw new Error('Registry URL must use https (http is allowed only for localhost)');
  }
  return url;
}

async function fetchReleaseDescriptor(registryUrl: string, id: string, version?: string): Promise<ReleaseDescriptor> {
  const url = registryEndpoint(registryUrl, id, '/release');
  if (version) url.searchParams.set('version', version);
  const response = await fetch(url, { headers: { accept: 'application/json' }, signal: AbortSignal.timeout(15_000) });
  if (response.status === 404) throw new Error(`No verified release found for ${id}`);
  if (!response.ok) throw new Error(`Registry returned ${response.status}`);
  const descriptor = (await response.json()) as ReleaseDescriptor;
  if (descriptor.id !== id) throw new Error('Registry returned a release for a different miniapp');
  if (descriptor.manifest?.id && descriptor.manifest.id !== id) {
    throw new Error('Release manifest id does not match the requested miniapp');
  }
  if (!descriptor.version || !VERSION_PATTERN.test(descriptor.version) || descriptor.version.includes('..')) {
    throw new Error(`Release version is invalid: ${descriptor.version}`);
  }
  return descriptor;
}

async function reportInstallEvent(
  registryUrl: string,
  id: string,
  event: 'install' | 'update' | 'rollback' | 'uninstall',
  version: string,
): Promise<void> {
  try {
    const url = registryEndpoint(registryUrl, id, '/install-events');
    await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ version, event, platform: process.platform, clientVersion: app.getVersion() }),
      signal: AbortSignal.timeout(5_000),
    });
  } catch (error) {
    log.warn(`[mini-app-releases] failed to report ${event} for ${id}:`, error);
  }
}

// ─── Download with checksum verification ─────────────────────────────────────

async function downloadArchive(asset: ReleaseAssetInfo, destination: string): Promise<void> {
  if (!asset.downloadUrl) throw new Error('Release archive has no download URL');
  if (asset.sizeBytes <= 0 || asset.sizeBytes > MAX_ARCHIVE_BYTES) {
    throw new Error(`Release archive size is out of bounds: ${asset.sizeBytes}`);
  }
  const response = await fetch(asset.downloadUrl, { signal: AbortSignal.timeout(10 * 60_000) });
  if (!response.ok || !response.body) throw new Error(`Archive download failed with status ${response.status}`);
  const hash = createHash('sha256');
  let received = 0;
  const meter = new PassThrough();
  meter.on('data', (chunk: Buffer) => {
    received += chunk.length;
    if (received > asset.sizeBytes) meter.destroy(new Error('Archive is larger than declared'));
    hash.update(chunk);
  });
  const source = Readable.fromWeb(response.body as unknown as import('node:stream/web').ReadableStream);
  await pipeline(source, meter, fs.createWriteStream(destination, { mode: 0o600 }));
  if (received !== asset.sizeBytes) {
    throw new Error(`Archive size mismatch: expected ${asset.sizeBytes} bytes, got ${received}`);
  }
  if (hash.digest('hex') !== asset.sha256) {
    throw new Error('Archive checksum mismatch');
  }
}

// ─── Archive extraction with traversal protection ────────────────────────────

interface ToolOutput {
  code: number | null;
  stdout: string;
  stderr: string;
  error?: Error;
}

function runTool(binary: string, args: string[], timeoutMs = 120_000): Promise<ToolOutput> {
  return new Promise((resolve) => {
    const proc = spawn(binary, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    let settled = false;
    const finish = (output: ToolOutput) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(output);
    };
    const timer = setTimeout(() => {
      proc.kill('SIGKILL');
      finish({ code: null, stdout, stderr: `${stderr}\nTimed out after ${timeoutMs}ms` });
    }, timeoutMs);
    proc.stdout.on('data', (chunk: Buffer) => {
      if (stdout.length < MAX_LISTING_BYTES) stdout += chunk.toString();
    });
    proc.stderr.on('data', (chunk: Buffer) => {
      if (stderr.length < 1024 * 1024) stderr += chunk.toString();
    });
    proc.on('error', (error) => finish({ code: null, stdout, stderr, error }));
    proc.on('close', (code) => finish({ code, stdout, stderr }));
  });
}

/** Validate a `tar -tf` listing: every entry must stay inside the target dir. */
export function validateArchiveEntries(listing: string): { entries: string[]; error?: string } {
  const entries = listing.split('\n').map((line) => line.trim()).filter(Boolean);
  if (!entries.length) return { entries, error: 'Archive is empty' };
  if (entries.length > MAX_ARCHIVE_ENTRIES) {
    return { entries, error: `Archive has too many entries (${entries.length})` };
  }
  for (const entry of entries) {
    if (entry.includes('\0')) return { entries, error: 'Archive entry contains a null byte' };
    if (entry.startsWith('/') || /^[A-Za-z]:[\\/]/.test(entry) || entry.startsWith('\\\\')) {
      return { entries, error: `Archive entry escapes the install directory: ${entry}` };
    }
    if (entry.split('/').includes('..')) {
      return { entries, error: `Archive entry contains '..': ${entry}` };
    }
    if (entry.includes('\\')) {
      return { entries, error: `Archive entry contains a backslash: ${entry}` };
    }
  }
  return { entries };
}

/**
 * Walk an extracted tree: reject symlinks, non-file/non-directory entries,
 * and archives that expand beyond the size cap. With no symlinks present,
 * everything is guaranteed to stay under the extraction root.
 */
export function validateExtractedTree(root: string): string | null {
  let entries = 0;
  let totalBytes = 0;
  const pending = [root];
  while (pending.length) {
    const directory = pending.pop() as string;
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const full = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) return `Archive created a symbolic link: ${entry.name}`;
      if (entry.isDirectory()) {
        pending.push(full);
      } else if (entry.isFile()) {
        totalBytes += fs.statSync(full).size;
        if (totalBytes > MAX_EXTRACTED_BYTES) return 'Archive expands beyond the size limit';
      } else {
        return `Archive created an unsupported entry type: ${entry.name}`;
      }
      entries += 1;
      if (entries > MAX_ARCHIVE_ENTRIES) return 'Archive extracted too many entries';
    }
  }
  return null;
}

async function extractArchive(archivePath: string, mime: string, stagingDir: string): Promise<string | null> {
  if (mime === 'application/zip' && process.platform === 'linux') {
    // GNU tar cannot read zip; extract in pure JS with the same traversal checks.
    try {
      const zipError = extractZipSafely(archivePath, stagingDir, {
        maxEntries: MAX_ARCHIVE_ENTRIES,
        maxBytes: MAX_EXTRACTED_BYTES,
      });
      return zipError ?? validateExtractedTree(stagingDir);
    } catch (err) {
      return `Could not extract archive: ${err instanceof Error ? err.message : String(err)}`;
    }
  }
  const listing = await runTool('tar', ['-tf', archivePath]);
  if (listing.code !== 0) {
    return `Could not list archive: ${listing.stderr.trim() || listing.error?.message || 'unknown error'}`;
  }
  const { error } = validateArchiveEntries(listing.stdout);
  if (error) return error;
  fs.mkdirSync(stagingDir, { recursive: true, mode: 0o700 });
  const extract = await runTool('tar', ['-xf', archivePath, '-C', stagingDir, '--no-same-owner'], 5 * 60_000);
  if (extract.code !== 0) {
    return `Could not extract archive: ${extract.stderr.trim() || extract.error?.message || 'unknown error'}`;
  }
  return validateExtractedTree(stagingDir);
}

// ─── Sandboxed helpers ────────────────────────────────────────────────────────

interface CapturedRun {
  code: number | null;
  timedOut: boolean;
}

function runCaptured(
  binary: string,
  args: string[],
  options: { cwd: string; env: NodeJS.ProcessEnv; timeoutMs: number },
  onLine: (line: string, type: 'stdout' | 'stderr') => void,
): Promise<CapturedRun> {
  return new Promise((resolve) => {
    const proc = spawn(binary, args, { cwd: options.cwd, env: options.env });
    let settled = false;
    const finish = (result: CapturedRun) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(result);
    };
    const timer = setTimeout(() => {
      proc.kill('SIGKILL');
      finish({ code: null, timedOut: true });
    }, options.timeoutMs);
    const forward = (chunk: Buffer, type: 'stdout' | 'stderr') => {
      for (const line of chunk.toString().split('\n')) {
        if (line.trim()) onLine(line.trim(), type);
      }
    };
    proc.stdout?.on('data', (chunk: Buffer) => forward(chunk, 'stdout'));
    proc.stderr?.on('data', (chunk: Buffer) => forward(chunk, 'stderr'));
    proc.on('error', () => finish({ code: null, timedOut: false }));
    proc.on('close', (code) => finish({ code, timedOut: false }));
  });
}

/**
 * Install npm dependencies inside the version directory. Lifecycle scripts
 * are disabled (--ignore-scripts) and the whole step runs under the platform
 * sandbox; it requires declared network access because npm must reach a
 * registry. Self-contained bundles (no dependencies) skip this step.
 */
async function installDependencies(
  versionDir: string,
  appDir: string,
  permissions: MiniAppSandboxPermissions,
  onLine: (line: string, type: 'stdout' | 'stderr') => void,
): Promise<string | null> {
  const packagePath = path.join(versionDir, 'package.json');
  if (!fs.existsSync(packagePath)) return null;
  let pkg: { dependencies?: Record<string, string> };
  try {
    pkg = JSON.parse(fs.readFileSync(packagePath, 'utf8'));
  } catch {
    return 'package.json is not valid JSON';
  }
  if (!Object.keys(pkg.dependencies || {}).length) return null;
  if (!permissions.network?.length) {
    return 'This release has npm dependencies but the manifest declares no network access';
  }
  const cacheDir = path.join(appDir, 'quarantine', 'npm-cache');
  fs.mkdirSync(path.join(cacheDir, 'tmp'), { recursive: true });
  // Dependency resolution legitimately needs package registries beyond the
  // runtime host list; steady-state execution never gets full network.
  const command = sandboxCommand(
    'npm',
    ['ci', '--ignore-scripts', '--omit=dev', '--no-audit', '--no-fund', '--cache', cacheDir],
    versionDir,
    { ...permissions, filesystem: [...(permissions.filesystem || []), cacheDir] },
    'full',
  );
  if ('error' in command) return command.error;
  const env: NodeJS.ProcessEnv = {
    PATH: process.env.PATH || '',
    HOME: cacheDir,
    TMPDIR: path.join(cacheDir, 'tmp'),
    npm_config_cache: cacheDir,
  };
  const result = await runCaptured(command.binary, command.args, { cwd: versionDir, env, timeoutMs: 5 * 60_000 }, onLine);
  if (result.code !== 0) {
    return result.timedOut ? 'npm ci timed out after 5 minutes' : `npm ci failed (exit ${result.code})`;
  }
  return null;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function httpOk(url: string): Promise<boolean> {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(2_000), redirect: 'manual' });
    return response.status < 500;
  } catch {
    return false;
  }
}

/**
 * Verify a freshly switched release through the `current` link. Declared
 * secrets are injected only when already stored; if any are missing the
 * start-based checks are skipped (the app can be verified after the user
 * configures secrets) and the install completes marked unverified.
 */
async function verifyHealthy(
  id: string,
  currentDir: string,
  manifest: MiniAppReleaseManifest,
  onLine: (line: string, type: 'stdout' | 'stderr' | 'info') => void,
): Promise<string | null> {
  const health = manifest.lifecycle?.health;
  if (!health) return null;
  const permissions = manifest.permissions || {};
  const declaredSecrets = manifest.permissions?.secrets || [];
  const missingSecrets = declaredSecrets.filter((name) => !listMiniAppSecrets(id).includes(name));
  if (missingSecrets.length) {
    onLine(`Skipping health checks until secrets are configured: ${missingSecrets.join(', ')}`, 'info');
    return null;
  }
  const env: NodeJS.ProcessEnv = {};
  for (const name of ['PATH', 'HOME', 'USER', 'SHELL', 'TMPDIR', 'LANG']) {
    if (process.env[name]) env[name] = process.env[name];
  }
  // Firmlinked TMPDIR (/var/folders/…) canonicalized to the backing-store
  // form as belt-and-braces (profile ancestor literals already make the
  // firmlink form traversable).
  if (env.TMPDIR) {
    try {
      env.TMPDIR = fs.realpathSync(env.TMPDIR);
    } catch { /* keep the original value */ }
  }
  Object.assign(env, getMiniAppSecretEnvironment(id, declaredSecrets), { ALLTERNIT_MINIAPP_ID: id });

  if (health.kind === 'command') {
    if (!health.command) return 'Health check declares kind=command without a command';
    const command = sandboxCommand('/bin/sh', ['-lc', health.command, ...(health.args || [])], currentDir, permissions);
    if ('error' in command) return command.error;
    const result = await runCaptured(command.binary, command.args, { cwd: currentDir, env, timeoutMs: 60_000 }, onLine);
    return result.code === 0 ? null : `Health check command exited with ${result.code}`;
  }

  if (health.kind === 'http' || health.kind === 'process') {
    const start = manifest.lifecycle?.start;
    if (!start?.command) return `Health check kind=${health.kind} requires lifecycle.start.command`;
    if (health.kind === 'http' && !permissions.network?.length) {
      return 'An http health check requires network permission in the manifest';
    }
    if (health.kind === 'http' && !health.url) return 'Health check declares kind=http without a url';
    const command = sandboxCommand('/bin/sh', ['-lc', start.command, ...(start.args || [])], currentDir, permissions);
    if ('error' in command) return command.error;
    // The service under verification gets loopback-only sandbox network
    // access; declared external hosts are reachable through the policy proxy.
    let proxy: Awaited<ReturnType<typeof startMiniAppPolicyProxy>> | undefined;
    if (permissions.network?.length) {
      try {
        proxy = await startMiniAppPolicyProxy({ appId: id, allowedHosts: permissions.network });
      } catch (error) {
        return `Network policy proxy failed to start: ${error instanceof Error ? error.message : String(error)}`;
      }
    }
    const child = spawn(command.binary, command.args, {
      cwd: currentDir,
      env: { ...env, ...(proxy?.environment || {}) },
      stdio: 'ignore',
    });
    try {
      if (health.kind === 'process') {
        await delay(3_000);
        return child.exitCode === null ? null : 'Process health check failed: the service exited immediately';
      }
      const deadline = Date.now() + 30_000;
      while (Date.now() < deadline) {
        if (child.exitCode !== null) return 'The service exited before becoming healthy';
        if (await httpOk(health.url as string)) return null;
        await delay(500);
      }
      return 'The service did not become healthy within 30 seconds';
    } finally {
      await proxy?.close();
      try {
        child.kill('SIGTERM');
      } catch {
        /* already gone */
      }
    }
  }

  return `Unsupported health check kind: ${(health as { kind: string }).kind}`;
}

// ─── Atomic switch ────────────────────────────────────────────────────────────

/**
 * Point `current` at versions/<version> atomically: build a new symlink and
 * rename it over the old one. POSIX rename is atomic; on Windows (where
 * community sandboxing fails closed anyway) fall back to unlink + rename.
 */
export function switchCurrent(appDir: string, version: string): void {
  const temporary = path.join(appDir, `.current-${process.pid}-${Date.now()}`);
  fs.rmSync(temporary, { force: true });
  fs.symlinkSync(path.join('versions', version), temporary, 'junction');
  const current = path.join(appDir, 'current');
  try {
    fs.renameSync(temporary, current);
  } catch (error) {
    if (process.platform !== 'win32') throw error;
    fs.rmSync(current, { force: true });
    fs.renameSync(temporary, current);
  }
}

// ─── Public API ───────────────────────────────────────────────────────────────

export async function installReleaseFromRegistry(
  options: { registryUrl: string; id: string; version?: string },
  onProgress: (progress: ReleaseInstallProgress) => void = () => undefined,
): Promise<ReleaseInstallResult> {
  const { registryUrl, id, version } = options;
  const info = (line: string) => onProgress({ id, line, type: 'info' });
  const output = (line: string, type: 'stdout' | 'stderr') => onProgress({ id, line, type });
  const appDir = appDirFor(id);
  const quarantineDir = path.join(appDir, 'quarantine');
  try {
    if (!MINIAPP_ID_PATTERN.test(id)) return { success: false, id, error: 'Invalid miniapp identifier' };

    info('Fetching release descriptor…');
    const descriptor = await fetchReleaseDescriptor(registryUrl, id, version);
    const targetVersion = descriptor.version;

    const platforms = descriptor.manifest?.compatibility?.platforms;
    if (platforms?.length && !platforms.includes(process.platform)) {
      return { success: false, id, version: targetVersion, error: `This release is not available on ${process.platform}` };
    }

    if (!descriptor.signature || !descriptor.publisherKey) {
      return { success: false, id, version: targetVersion, error: 'Release is not signed' };
    }
    if (!verifyManifestSignature(descriptor.manifest, descriptor.signature, descriptor.publisherKey)) {
      return { success: false, id, version: targetVersion, error: 'Release signature verification failed' };
    }
    info('✓ Publisher signature verified');

    const archive = descriptor.assets.find((asset) => asset.kind === 'archive');
    if (!archive) return { success: false, id, version: targetVersion, error: 'Release has no archive asset' };

    fs.rmSync(quarantineDir, { recursive: true, force: true });
    fs.mkdirSync(quarantineDir, { recursive: true, mode: 0o700 });
    const versionsDir = path.join(appDir, 'versions');
    fs.mkdirSync(versionsDir, { recursive: true });
    for (const stale of fs.readdirSync(versionsDir)) {
      if (stale.startsWith('.staging-')) fs.rmSync(path.join(versionsDir, stale), { recursive: true, force: true });
    }

    info(`Downloading release ${targetVersion}…`);
    const archivePath = path.join(quarantineDir, 'release.archive');
    await downloadArchive(archive, archivePath);
    info('✓ Archive checksum verified');

    const versionDir = path.join(versionsDir, targetVersion);
    const markerPath = path.join(versionDir, INSTALL_MARKER);
    if (fs.existsSync(versionDir) && !fs.existsSync(markerPath)) {
      // A partial earlier attempt; wipe and re-extract.
      fs.rmSync(versionDir, { recursive: true, force: true });
    }
    if (!fs.existsSync(versionDir)) {
      info('Extracting archive…');
      const stagingDir = path.join(versionsDir, `.staging-${process.pid}-${Date.now()}`);
      const extractError = await extractArchive(archivePath, archive.mime, stagingDir);
      if (extractError) {
        fs.rmSync(stagingDir, { recursive: true, force: true });
        return { success: false, id, version: targetVersion, error: extractError };
      }
      fs.renameSync(stagingDir, versionDir);
      fs.writeFileSync(
        markerPath,
        JSON.stringify({ version: targetVersion, sha256: archive.sha256, installedAt: new Date().toISOString() }),
        { mode: 0o600 },
      );
    } else {
      info('This release is already extracted; reusing it');
    }

    info('Installing dependencies…');
    const dependenciesError = await installDependencies(versionDir, appDir, descriptor.manifest?.permissions || {}, output);
    if (dependenciesError) return { success: false, id, version: targetVersion, error: dependenciesError };

    const priorState = loadState(appDir);
    switchCurrent(appDir, targetVersion);

    info('Running health checks…');
    const healthError = await verifyHealthy(id, path.join(appDir, 'current'), descriptor.manifest || {}, (line, type) =>
      onProgress({ id, line, type: type === 'info' ? 'info' : type }),
    );
    if (healthError) {
      if (priorState.currentVersion && fs.existsSync(path.join(versionsDir, priorState.currentVersion))) {
        switchCurrent(appDir, priorState.currentVersion);
        void reportInstallEvent(registryUrl, id, 'rollback', priorState.currentVersion);
      }
      fs.rmSync(versionDir, { recursive: true, force: true });
      return {
        success: false,
        id,
        version: targetVersion,
        previousVersion: priorState.currentVersion,
        rolledBack: Boolean(priorState.currentVersion),
        error: healthError,
      };
    }

    const nextState: ReleaseInstallState = {
      id,
      currentVersion: targetVersion,
      previousVersion:
        priorState.currentVersion && priorState.currentVersion !== targetVersion
          ? priorState.currentVersion
          : priorState.previousVersion,
      releases: {
        ...priorState.releases,
        [targetVersion]: {
          installedAt: new Date().toISOString(),
          sha256: archive.sha256,
          signature: descriptor.signature,
          publisherKey: descriptor.publisherKey,
          healthy: !healthError,
        },
      },
    };
    saveState(appDir, nextState);
    void reportInstallEvent(registryUrl, id, priorState.currentVersion && priorState.currentVersion !== targetVersion ? 'update' : 'install', targetVersion);
    info(`✓ ${id} ${targetVersion} installed`);
    return { success: true, id, version: targetVersion, previousVersion: nextState.previousVersion };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    log.warn(`[mini-app-releases] install of ${id} failed: ${message}`);
    return { success: false, id, error: message };
  } finally {
    fs.rmSync(quarantineDir, { recursive: true, force: true });
  }
}

/** Switch `current` back to the previously installed version. */
export function rollbackReleaseInstall(
  id: string,
  registryUrl?: string,
): { success: boolean; error?: string; currentVersion?: string } {
  const appDir = appDirFor(id);
  const state = loadState(appDir);
  const target = state.previousVersion;
  if (!target || !fs.existsSync(path.join(appDir, 'versions', target))) {
    return { success: false, error: 'No previous version is available' };
  }
  switchCurrent(appDir, target);
  const next: ReleaseInstallState = {
    ...state,
    currentVersion: target,
    previousVersion: state.currentVersion && state.currentVersion !== target ? state.currentVersion : state.previousVersion,
  };
  saveState(appDir, next);
  if (registryUrl) void reportInstallEvent(registryUrl, id, 'rollback', target);
  return { success: true, currentVersion: target };
}

/** Remove the whole versioned install (caller stops running processes first). */
export function removeReleaseInstall(id: string, registryUrl?: string): { success: boolean; error?: string } {
  try {
    const appDir = appDirFor(id);
    const state = loadState(appDir);
    fs.rmSync(appDir, { recursive: true, force: true });
    if (registryUrl && state.currentVersion) void reportInstallEvent(registryUrl, id, 'uninstall', state.currentVersion);
    return { success: true };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : String(error) };
  }
}

export function getReleaseInstallState(id: string): ReleaseInstallInfo | null {
  const state = loadState(appDirFor(id));
  if (!state.id && !state.currentVersion) return null;
  return {
    id,
    currentVersion: state.currentVersion,
    previousVersion: state.previousVersion,
    healthy: state.currentVersion ? state.releases[state.currentVersion]?.healthy ?? false : false,
    releases: state.releases,
  };
}

export function listReleaseInstalls(): ReleaseInstallInfo[] {
  const root = releasesRoot();
  if (!fs.existsSync(root)) return [];
  const installs: ReleaseInstallInfo[] = [];
  for (const entry of fs.readdirSync(root)) {
    try {
      const state = loadState(path.join(root, entry));
      if (!state.id) continue;
      installs.push({
        id: state.id,
        currentVersion: state.currentVersion,
        previousVersion: state.previousVersion,
        healthy: state.currentVersion ? state.releases[state.currentVersion]?.healthy ?? false : false,
        releases: state.releases,
      });
    } catch {
      continue;
    }
  }
  return installs.sort((left, right) => left.id.localeCompare(right.id));
}
