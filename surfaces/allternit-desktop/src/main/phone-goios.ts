/**
 * go-ios for Phone Link (iPhone): resolve or download the pinned binary and
 * wrap the commands the setup uses.
 *
 * go-ios (MIT, github.com/danielpaulus/go-ios) is never bundled: a pinned
 * release asset is downloaded on first iPhone setup and checked against the
 * SHA-256 GitHub publishes for it, like ensureAdb does for platform-tools.
 * macOS only for now (the iPhone route needs a Mac, see phone-link.ts).
 *
 * Output: go-ios logs JSON lines ({"level","msg",…}) on stderr and prints
 * command results as JSON on stdout. Long-lived commands (tunnel, runtest)
 * go through spawnSidecar so they die with the app, even on a crash.
 *
 * Not used: `ios forward` (binds 0.0.0.0, see phone-usbmux.ts) and
 * `ios image auto`'s own download (third-party mirror, see ensureDdi).
 */

import { execFile } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { sha256Hex } from './phone-adb.js';
import { spawnSidecar } from './process-lifeline.js';
import type { FetchFn } from './ictrl-client.js';

export const GO_IOS_VERSION = 'v1.3.2';

export interface PinnedArchive {
  url: string;
  sha256: string;
  size: number;
}

/** Digest from the GitHub release API (assets[].digest) for go-ios-mac.zip, v1.3.2. */
export const GO_IOS_DARWIN: PinnedArchive = {
  url: `https://github.com/danielpaulus/go-ios/releases/download/${GO_IOS_VERSION}/go-ios-mac.zip`,
  sha256: '100f225bfdd039081bcbdaf45029df3e67032673f3e0ca1fa3c050898c230c57',
  size: 16_821_315,
};

/**
 * The personalized Developer Disk Image go-ios mounts on iOS 17+ (it only
 * ever uses this one, "ddi-17E5179g", Xcode 15.4's). go-ios downloads it from
 * deviceboxhq.com unless <basedir>/ddi-17E5179g.zip is already there, so we
 * put a SHA-pinned copy there first. It is Apple software: hosting it on
 * runtime.allternit.com needs a licensing decision (see the PR notes);
 * until then a Mac with Xcode can supply it locally (resolveLocalDdi).
 */
export const DDI_NAME = 'ddi-17E5179g';
export const DDI_ARCHIVE: PinnedArchive = {
  url: `https://runtime.allternit.com/phone-link/ddi/${DDI_NAME}.zip`,
  sha256: 'b74e5d038df88fcf954f2a503bcbeb94a3fb78ad508a557665ac6bf325bece1a',
  size: 11_508_236,
};

export function verifyPinned(data: Buffer, archive: PinnedArchive): boolean {
  return data.length === archive.size && sha256Hex(data) === archive.sha256;
}

export function goIosDir(dataDir: string): string {
  return path.join(dataDir, 'go-ios', GO_IOS_VERSION);
}

export function resolveGoIos(dataDir: string): string | null {
  for (const candidate of [process.env.ALLTERNIT_GOIOS_PATH, path.join(goIosDir(dataDir), 'ios')]) {
    if (candidate && fs.existsSync(candidate)) return candidate;
  }
  return null;
}

function run(command: string, args: string[]): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(command, args, (error, _stdout, stderr) => (error ? reject(new Error(`${command} failed: ${stderr || error.message}`)) : resolve()));
  });
}

export interface DownloadDeps {
  fetchImpl?: FetchFn;
  extract?: (zipPath: string, destDir: string) => Promise<void>;
  platform?: NodeJS.Platform;
}

async function downloadPinned(archive: PinnedArchive, dest: string, fetchImpl: FetchFn, what: string): Promise<void> {
  const response = await fetchImpl(archive.url);
  if (!response.ok) throw new Error(`Downloading ${what} failed: HTTP ${response.status}`);
  const data = Buffer.from(await response.arrayBuffer());
  if (!verifyPinned(data, archive)) throw new Error(`Downloaded ${what} did not match the pinned checksum; refusing to use it.`);
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(dest, data);
}

/** A usable `ios` binary, downloading and verifying go-ios if needed. */
export async function ensureGoIos(dataDir: string, deps: DownloadDeps = {}): Promise<string> {
  const platform = deps.platform ?? process.platform;
  const existing = resolveGoIos(dataDir);
  if (existing) return existing;
  if (platform !== 'darwin') throw new Error('iPhone setup needs a Mac.');
  const dir = goIosDir(dataDir);
  const zip = path.join(dir, 'go-ios-mac.zip');
  await downloadPinned(GO_IOS_DARWIN, zip, deps.fetchImpl ?? fetch, 'go-ios');
  await (deps.extract ?? ((z, d) => run('unzip', ['-q', '-o', z, '-d', d])))(zip, dir);
  fs.rmSync(zip, { force: true });
  const bin = path.join(dir, 'ios');
  if (!fs.existsSync(bin)) throw new Error('go-ios unpacked but the ios binary was not found.');
  fs.chmodSync(bin, 0o755);
  return bin;
}

/** go-ios's own source, used only as a fallback and only checked against the same pin. */
export const DDI_FALLBACK_URL = `https://deviceboxhq.com/${DDI_NAME}.zip`;

/**
 * Put the pinned DDI where `ios image auto --basedir` finds it, so go-ios
 * never downloads its own. Ours first, then go-ios's mirror; either way the
 * bytes must match the pin, so the mirror only affects availability.
 */
export async function ensureDdi(basedir: string, deps: DownloadDeps & { archive?: PinnedArchive; fallbackUrls?: string[] } = {}): Promise<void> {
  const zip = path.join(basedir, `${DDI_NAME}.zip`);
  const archive = deps.archive ?? DDI_ARCHIVE;
  // go-ios prefers any extracted Restore/ under basedir over the zip, so only
  // the verified zip may stay; it re-extracts it (11 MB) on each mount.
  fs.mkdirSync(basedir, { recursive: true });
  for (const entry of fs.readdirSync(basedir)) {
    if (entry !== `${DDI_NAME}.zip`) fs.rmSync(path.join(basedir, entry), { recursive: true, force: true });
  }
  if (fs.existsSync(zip) && verifyPinned(fs.readFileSync(zip), archive)) return;
  fs.rmSync(zip, { force: true });
  const urls = [archive.url, ...(deps.fallbackUrls ?? [DDI_FALLBACK_URL])];
  let lastError: unknown;
  for (const url of urls) {
    try {
      await downloadPinned({ ...archive, url }, zip, deps.fetchImpl ?? fetch, 'the developer disk image');
      return;
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError));
}

// ─── Output parsing ──────────────────────────────────────────────────────────

export interface GoIosLogLine {
  level?: string;
  msg?: string;
  err?: string;
  [key: string]: unknown;
}

/** Every JSON object line; other lines are skipped. */
export function parseJsonLines(text: string): GoIosLogLine[] {
  const out: GoIosLogLine[] = [];
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed.startsWith('{')) continue;
    try {
      const value = JSON.parse(trimmed);
      if (value && typeof value === 'object' && !Array.isArray(value)) out.push(value as GoIosLogLine);
    } catch {
      /* partial line */
    }
  }
  return out;
}

/** The last error go-ios logged (exitIfError → level ERROR/fatal), as "msg: err". */
export function lastGoIosError(text: string): string | null {
  const errors = parseJsonLines(text).filter((l) => /^(error|fatal)$/i.test(String(l.level ?? '')));
  const last = errors[errors.length - 1];
  if (!last) return text.trim().split('\n').pop()?.trim() || null;
  return [last.msg, last.err].filter(Boolean).join(': ');
}

/** The first stdout line that parses as JSON and isn't a log line (go-ios prints one result object). */
export function firstJson<T>(stdout: string): T | null {
  for (const line of stdout.split('\n')) {
    const t = line.trim();
    if (!t.startsWith('{') && !t.startsWith('[')) continue;
    try {
      const value = JSON.parse(t);
      if (value && !Array.isArray(value) && typeof value.level === 'string' && typeof value.msg === 'string') continue;
      return value as T;
    } catch {
      /* keep looking */
    }
  }
  return null;
}

/** `ios apps --list`: "<bundle id> <name…> <version>" per line. */
export function parseAppsList(stdout: string): string[] {
  return stdout
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith('{'))
    .map((l) => l.split(/\s+/)[0]);
}

// ─── Runner ──────────────────────────────────────────────────────────────────

export interface ExecResult {
  stdout: string;
  stderr: string;
  code: number;
}

export type GoIosExec = (args: string[], opts?: { timeoutMs?: number }) => Promise<ExecResult>;

export interface LongRunning {
  /** Each complete output line (stdout and stderr). */
  onLine(handler: (line: string, parsed: GoIosLogLine | null) => void): () => void;
  exited: Promise<{ code: number | null; signal: string | null }>;
  isRunning(): boolean;
  stop(): void;
}

export type GoIosSpawn = (args: string[]) => LongRunning;

export function createGoIosExec(bin: string): GoIosExec {
  return (args, opts = {}) =>
    new Promise((resolve) => {
      execFile(bin, args, { timeout: opts.timeoutMs ?? 60_000, maxBuffer: 32 * 1024 * 1024 }, (error, stdout, stderr) => {
        const code = error ? (typeof (error as { code?: unknown }).code === 'number' ? (error as { code: number }).code : 1) : 0;
        resolve({ stdout: String(stdout), stderr: String(stderr), code });
      });
    });
}

/** Long-lived go-ios command tied to this process's lifetime (process-lifeline.ts). */
export function createGoIosSpawn(bin: string, cwd: string): GoIosSpawn {
  return (args) => wrapChild(spawnSidecar(bin, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'] }));
}

export function wrapChild(child: ChildProcess): LongRunning {
  const handlers = new Set<(line: string, parsed: GoIosLogLine | null) => void>();
  let running = true;
  const feed = (stream: NodeJS.ReadableStream | null) => {
    let pending = '';
    stream?.on('data', (chunk: Buffer) => {
      pending += chunk.toString('utf8');
      const lines = pending.split('\n');
      pending = lines.pop() ?? '';
      for (const line of lines) {
        if (!line.trim()) continue;
        const parsed = parseJsonLines(line)[0] ?? null;
        for (const h of handlers) h(line, parsed);
      }
    });
  };
  feed(child.stdout);
  feed(child.stderr);
  const exited = new Promise<{ code: number | null; signal: string | null }>((resolve) => {
    child.once('exit', (code, signal) => {
      running = false;
      resolve({ code, signal });
    });
    child.once('error', () => {
      running = false;
      resolve({ code: null, signal: null });
    });
  });
  return {
    onLine(handler) {
      handlers.add(handler);
      return () => handlers.delete(handler);
    },
    exited,
    isRunning: () => running,
    stop() {
      if (running) child.kill('SIGTERM');
    },
  };
}

export interface TunnelInfo {
  udid: string;
  address?: string;
  rsdPort?: number;
  userspaceTUN?: boolean;
  userspaceTUNPort?: number;
}

export interface RunTestOptions {
  udid: string;
  runnerBundleId: string;
  xctestConfig: string;
  testToRun: string;
  env?: Record<string, string>;
}

export type LaunchResult = { ok: true } | { ok: false; error: string; untrusted: boolean; notInstalled: boolean };

/** The phrases iOS/go-ios use when the developer certificate isn't trusted yet. */
const UNTRUSTED = /explicitly trusted|untrusted developer|not been trusted|invalid code signature|0xe8008018|0xe800801c|profile has not/i;
const NOT_INSTALLED = /not installed|no such app|could not find|unknown bundle/i;

/** Typed go-ios commands. Every call is scoped to one UDID. */
export class GoIos {
  constructor(
    private readonly exec: GoIosExec,
    private readonly spawn: GoIosSpawn,
    /** Writable directory for go-ios's own tunnel pair identity (selfIdentity.plist). */
    private readonly pairRecordDir: string,
  ) {}

  private async json<T>(args: string[], timeoutMs?: number): Promise<T> {
    const res = await this.exec(args, { timeoutMs });
    if (res.code !== 0) throw new Error(lastGoIosError(res.stderr || res.stdout) ?? `ios ${args[0]} failed`);
    const parsed = firstJson<T>(res.stdout);
    if (parsed === null) throw new Error(`ios ${args[0]}: no JSON result`);
    return parsed;
  }

  /** UDIDs usbmuxd sees (plugged in, trusted or not). */
  async list(): Promise<string[]> {
    const res = await this.exec(['list'], { timeoutMs: 10_000 });
    if (res.code !== 0) return [];
    return firstJson<{ deviceList?: string[] }>(res.stdout)?.deviceList ?? [];
  }

  /** Lockdown values; fails while the phone hasn't trusted this Mac. */
  async info(udid: string): Promise<{ ok: true; name?: string; version?: string; productType?: string } | { ok: false; error: string }> {
    const res = await this.exec(['info', `--udid=${udid}`], { timeoutMs: 20_000 });
    if (res.code !== 0) return { ok: false, error: lastGoIosError(res.stderr || res.stdout) ?? 'info failed' };
    const v = firstJson<Record<string, unknown>>(res.stdout) ?? {};
    return {
      ok: true,
      name: typeof v.DeviceName === 'string' ? v.DeviceName : undefined,
      version: typeof v.ProductVersion === 'string' ? v.ProductVersion : undefined,
      productType: typeof v.ProductType === 'string' ? v.ProductType : undefined,
    };
  }

  /** Ask the phone to trust this Mac (shows "Trust This Computer?"). */
  async pair(udid: string): Promise<{ ok: boolean; error?: string }> {
    const res = await this.exec(['pair', `--udid=${udid}`], { timeoutMs: 30_000 });
    return res.code === 0 ? { ok: true } : { ok: false, error: lastGoIosError(res.stderr || res.stdout) ?? 'pair failed' };
  }

  async devmodeEnabled(udid: string): Promise<boolean> {
    const v = await this.json<{ DeveloperModeEnabled?: boolean }>(['devmode', 'get', `--udid=${udid}`], 15_000);
    return v.DeveloperModeEnabled === true;
  }

  /** Makes Settings › Privacy & Security › Developer Mode appear. */
  async devmodeReveal(udid: string): Promise<void> {
    await this.exec(['devmode', 'reveal', `--udid=${udid}`], { timeoutMs: 15_000 });
  }

  /** `ios tunnel start --userspace` (no sudo). Long-lived; serves the tunnel-info API on 127.0.0.1:28100. */
  tunnelStart(udid: string): LongRunning {
    return this.spawn(['tunnel', 'start', '--userspace', `--pair-record-path=${this.pairRecordDir}`, `--udid=${udid}`]);
  }

  async tunnels(): Promise<TunnelInfo[]> {
    const res = await this.exec(['tunnel', 'ls'], { timeoutMs: 5_000 });
    if (res.code !== 0) return [];
    const raw = firstJson<Record<string, unknown>[]>(res.stdout) ?? [];
    return raw.map((t) => ({
      udid: String(t.udid ?? t.Udid ?? ''),
      address: (t.address ?? t.Address) as string | undefined,
      rsdPort: (t.rsdPort ?? t.RsdPort) as number | undefined,
      userspaceTUN: (t.userspaceTun ?? t.UserspaceTUN) as boolean | undefined,
      userspaceTUNPort: (t.userspaceTunPort ?? t.UserspaceTUNPort) as number | undefined,
    }));
  }

  /** Mount the personalized DDI from `basedir` (ensureDdi first so nothing is fetched from a mirror). */
  async imageAuto(udid: string, basedir: string): Promise<void> {
    const res = await this.exec(['image', 'auto', `--basedir=${basedir}`, `--udid=${udid}`], { timeoutMs: 180_000 });
    if (res.code !== 0) throw new Error(lastGoIosError(res.stderr || res.stdout) ?? 'mounting the developer disk image failed');
  }

  async install(udid: string, ipaOrApp: string): Promise<void> {
    const res = await this.exec(['install', `--path=${ipaOrApp}`, `--udid=${udid}`], { timeoutMs: 300_000 });
    if (res.code !== 0) throw new Error(lastGoIosError(res.stderr || res.stdout) ?? 'install failed');
  }

  /** User-installed bundle ids (installation_proxy; no app is opened). */
  async apps(udid: string): Promise<string[]> {
    const res = await this.exec(['apps', '--list', `--udid=${udid}`], { timeoutMs: 30_000 });
    if (res.code !== 0) throw new Error(lastGoIosError(res.stderr || res.stdout) ?? 'listing apps failed');
    return parseAppsList(res.stdout);
  }

  /** A plain app launch (like tapping its icon): not tied to any test session or cable. */
  async launch(udid: string, bundleId: string, opts: { killExisting?: boolean } = {}): Promise<LaunchResult> {
    const args = ['launch', bundleId, `--udid=${udid}`];
    if (opts.killExisting) args.push('--kill-existing');
    const res = await this.exec(args, { timeoutMs: 30_000 });
    if (res.code === 0) return { ok: true };
    const error = lastGoIosError(res.stderr || res.stdout) ?? 'launch failed';
    return { ok: false, error, untrusted: UNTRUSTED.test(error), notInstalled: NOT_INSTALLED.test(error) };
  }

  async kill(udid: string, bundleId: string): Promise<void> {
    await this.exec(['kill', bundleId, `--udid=${udid}`], { timeoutMs: 15_000 });
  }

  /** Copy a file into an app's own container (house_arrest; works for our development-signed apps). */
  async pushToApp(udid: string, bundleId: string, local: string, remote: string): Promise<void> {
    const res = await this.exec(['file', 'push', `--app=${bundleId}`, `--local=${local}`, `--remote=${remote}`, `--udid=${udid}`], { timeoutMs: 30_000 });
    if (res.code !== 0) throw new Error(lastGoIosError(res.stderr || res.stdout) ?? 'file push failed');
  }

  /** XCUITest session for the runner. Long-lived. `env` reaches only this launch. */
  runtest(opts: RunTestOptions): LongRunning {
    const args = [
      'runtest',
      `--bundle-id=${opts.runnerBundleId}`,
      `--test-runner-bundle-id=${opts.runnerBundleId}`,
      `--xctest-config=${opts.xctestConfig}`,
      `--test-to-run=${opts.testToRun}`,
      `--udid=${opts.udid}`,
    ];
    for (const [k, v] of Object.entries(opts.env ?? {})) args.push(`--env=${k}=${v}`);
    return this.spawn(args);
  }
}
