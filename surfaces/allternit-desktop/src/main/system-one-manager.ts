/**
 * System One (S1) + Laya Manager (Q28, Q29)
 *
 * Supervises the local S1 decision runtime that ships with Desktop:
 *   - the bundled `system-one` server binary (bun --compile of
 *     tools/system-one-local/src/cli.ts) on 127.0.0.1:7717, with the shadow
 *     ledger on by default (Q28) and SYSTEM_ONE_LAYA_URL pointing at Laya;
 *   - Laya (convaiinnovations/laya, laya[serve]==0.3.22, base checkpoint pinned
 *     to rev 55cf4c4 until our fine-tuned revision passes Q26) on 127.0.0.1:7718,
 *     installed into ~/Library/Application Support/Allternit/laya.
 *
 * Laya is a dependency (Q29): on first run it installs in the background (uv venv
 * + pip) without blocking app start. uv is never bundled; without it the status
 * says "needs uv" with an install hint.
 *
 * Health: S1 GET /healthz, Laya GET /health. The local allternit-api gets
 * getApiEnvironment(): ALLTERNIT_S1_URL, and ALLTERNIT_S1_BACKEND=laya_bundled
 * once Laya has reported healthy (system_one_local until then).
 *
 * Mirrors bonsai-companion-manager.ts; dependencies are injectable for tests.
 */

import { app, BrowserWindow } from 'electron';
import { spawn as nodeSpawn, ChildProcess, SpawnOptions } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import log from 'electron-log';
import { spawnSidecar as lifelineSpawnSidecar } from './process-lifeline.js';

export const SYSTEM_ONE_DEFAULT_PORT = 7717;
export const LAYA_DEFAULT_PORT = 7718;
export const LAYA_VERSION = '0.3.22';
/** Q29: convaiinnovations/laya, typed-decisions, rev 55cf4c4. */
export const LAYA_PINNED_REVISION = '55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851';
export const UV_INSTALL_HINT =
  "Laya needs uv. Install it with 'brew install uv' or 'curl -LsSf https://astral.sh/uv/install.sh | sh', then choose Repair.";

const S1_HEALTH_TIMEOUT_MS = 30_000;
/** First Laya start downloads ~600 MB of weights. */
const LAYA_HEALTH_TIMEOUT_MS = 15 * 60_000;
const NEEDS_UV_EXIT = 3;

export type S1Backend = 'laya_bundled' | 'system_one_local';

/** Which Laya checkpoint to serve. Our fine-tuned revision swaps in here (WP-L1). */
export interface LayaCheckpoint {
  source: 'pinned' | 'revision' | 'path';
  /** Hugging Face revision (commit, branch or tag) of convaiinnovations/laya. */
  revision?: string;
  /** Local typed-decisions checkpoint directory. */
  path?: string;
}

export interface SystemOneStatus {
  /** Laya's scripts are bash; Windows runs S1 without Laya. */
  layaSupported: boolean;
  systemOne: {
    available: boolean;
    running: boolean;
    url: string;
  };
  laya: {
    installed: boolean;
    installing: boolean;
    running: boolean;
    url: string;
    version: string;
    checkpoint: LayaCheckpoint;
    needsUv: boolean;
    uvHint?: string;
    installDir: string;
  };
  /** S1 backend new allternit-api starts get (laya_bundled once Laya is healthy). */
  backend: S1Backend;
  /** Backend the running allternit-api was started with, if it was. */
  apiBackend?: S1Backend;
  /** Q28 shadow ledger directory, or null when opted out (SYSTEM_ONE_SHADOW_LOG=0). */
  shadowDir: string | null;
  error?: string;
}

export interface SystemOneProgress {
  stage: 'starting' | 'installing' | 'ready' | 'error' | 'cancelled' | 'needs-uv';
  message: string;
}

type SpawnFn = (command: string, args: readonly string[], options: SpawnOptions) => ChildProcess;

export interface SystemOneDeps {
  spawn: SpawnFn;
  spawnSidecar: SpawnFn;
  fetch: (input: string, init?: RequestInit) => Promise<Response>;
  env: NodeJS.ProcessEnv;
  platform: NodeJS.Platform;
  homedir: string;
  isPackaged: boolean;
  resourcesPath: string;
  appPath: string;
  emitProgress: (progress: SystemOneProgress) => void;
  sleep: (ms: number) => Promise<void>;
}

function electronDeps(): SystemOneDeps {
  return {
    spawn: nodeSpawn as SpawnFn,
    spawnSidecar: lifelineSpawnSidecar as SpawnFn,
    fetch: (input, init) => fetch(input, init),
    env: process.env,
    platform: process.platform,
    homedir: os.homedir(),
    isPackaged: app?.isPackaged ?? false,
    resourcesPath: process.resourcesPath ?? '',
    appPath: typeof app?.getAppPath === 'function' ? app.getAppPath() : process.cwd(),
    emitProgress: (progress) => {
      for (const win of BrowserWindow.getAllWindows()) {
        win.webContents.send('system-one:progress', progress);
      }
    },
    sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  };
}

export class SystemOneManager {
  private static instance: SystemOneManager | undefined;
  private s1Proc: ChildProcess | null = null;
  private layaProc: ChildProcess | null = null;
  private installProc: ChildProcess | null = null;
  private installPromise: Promise<void> | null = null;
  private layaHealthy = false;
  private needsUv = false;
  private lastError: string | undefined;
  private apiBackend: S1Backend | undefined;
  private stopped = false;
  private depsCache: SystemOneDeps | undefined;

  constructor(private readonly overrides: Partial<SystemOneDeps> = {}) {}

  static getInstance(): SystemOneManager {
    if (!SystemOneManager.instance) SystemOneManager.instance = new SystemOneManager();
    return SystemOneManager.instance;
  }

  /** Resolved lazily: app.getAppPath() is only valid once Electron is ready. */
  private get deps(): SystemOneDeps {
    if (!this.depsCache) this.depsCache = { ...electronDeps(), ...this.overrides };
    return this.depsCache;
  }

  // ── Paths & config ─────────────────────────────────────────────────────────

  get root(): string {
    return this.deps.env.ALLTERNIT_LAYA_HOME || this.defaultRoot();
  }

  private defaultRoot(): string {
    return path.join(this.deps.homedir, 'Library', 'Application Support', 'Allternit', 'laya');
  }

  get s1Port(): number {
    return Number(this.deps.env.ALLTERNIT_S1_PORT) || SYSTEM_ONE_DEFAULT_PORT;
  }

  get layaPort(): number {
    return Number(this.deps.env.ALLTERNIT_LAYA_PORT) || LAYA_DEFAULT_PORT;
  }

  get s1Url(): string {
    return `http://127.0.0.1:${this.s1Port}`;
  }

  get layaUrl(): string {
    return `http://127.0.0.1:${this.layaPort}`;
  }

  get layaSupported(): boolean {
    return this.deps.platform !== 'win32';
  }

  /** Q28: shadow ledger on by default; SYSTEM_ONE_SHADOW_LOG=0 opts out. Same dir the CLI defaults to. */
  get shadowDir(): string | null {
    const env = this.deps.env;
    if (env.ALLTERNIT_S1_SHADOW_DIR?.trim()) return env.ALLTERNIT_S1_SHADOW_DIR.trim();
    if (env.SYSTEM_ONE_SHADOW_LOG === '0') return null;
    return path.join(this.deps.homedir, '.allternit', 'system-one', 'shadow');
  }

  private get configPath(): string {
    return path.join(this.root, 'config.json');
  }

  /**
   * Checkpoint precedence: env (ALLTERNIT_LAYA_CHECKPOINT_PATH / ALLTERNIT_LAYA_REVISION),
   * then <root>/config.json {"checkpoint": {"revision"|"path"}}, then the Q29 pin.
   */
  getCheckpoint(): LayaCheckpoint {
    const env = this.deps.env;
    if (env.ALLTERNIT_LAYA_CHECKPOINT_PATH?.trim()) return { source: 'path', path: env.ALLTERNIT_LAYA_CHECKPOINT_PATH.trim() };
    if (env.ALLTERNIT_LAYA_REVISION?.trim()) return { source: 'revision', revision: env.ALLTERNIT_LAYA_REVISION.trim() };
    try {
      const cfg = JSON.parse(fs.readFileSync(this.configPath, 'utf8')) as { checkpoint?: { revision?: unknown; path?: unknown } };
      const cp = cfg.checkpoint ?? {};
      if (typeof cp.path === 'string' && cp.path.trim()) return { source: 'path', path: cp.path.trim() };
      if (typeof cp.revision === 'string' && cp.revision.trim()) return { source: 'revision', revision: cp.revision.trim() };
    } catch {
      // No config: pinned base checkpoint.
    }
    return { source: 'pinned', revision: LAYA_PINNED_REVISION };
  }

  /** Swap the served checkpoint (null = back to the Q29 pin). Restarts Laya if it is running. */
  async setCheckpoint(checkpoint: { revision?: string; path?: string } | null): Promise<LayaCheckpoint> {
    if (checkpoint?.path && !path.isAbsolute(checkpoint.path)) {
      throw new Error('Laya checkpoint path must be absolute.');
    }
    if (checkpoint?.revision && !/^[\w.\-/]+$/.test(checkpoint.revision)) {
      throw new Error('Laya checkpoint revision must be a commit, branch or tag.');
    }
    await fs.promises.mkdir(this.root, { recursive: true });
    let cfg: Record<string, unknown> = {};
    try {
      cfg = JSON.parse(await fs.promises.readFile(this.configPath, 'utf8'));
    } catch {
      cfg = {};
    }
    if (checkpoint && (checkpoint.path || checkpoint.revision)) {
      cfg.checkpoint = checkpoint.path ? { path: checkpoint.path } : { revision: checkpoint.revision };
    } else {
      delete cfg.checkpoint;
    }
    await fs.promises.writeFile(this.configPath, `${JSON.stringify(cfg, null, 2)}\n`);
    log.info('[SystemOne] Laya checkpoint set to', this.getCheckpoint());
    if (this.layaProc) {
      this.stopLaya();
      void this.startLaya().catch((err) => this.fail(`Laya restart failed: ${(err as Error).message}`));
    }
    return this.getCheckpoint();
  }

  private repoCandidates(): string[] {
    const out: string[] = [];
    let dir = this.deps.appPath;
    for (let i = 0; i < 5 && dir; i++) {
      out.push(dir);
      const parent = path.dirname(dir);
      if (parent === dir) break;
      dir = parent;
    }
    return out;
  }

  /** Command that runs the S1 server: the packaged binary, a staged dev binary, or bun + cli.ts in dev. */
  resolveSystemOneCommand(): { command: string; args: string[] } | null {
    const exe = this.deps.platform === 'win32' ? 'system-one.exe' : 'system-one';
    const serveArgs = ['serve', '--port', String(this.s1Port)];
    const binaries = [path.join(this.deps.resourcesPath, 'bin', exe)];
    if (!this.deps.isPackaged) {
      for (const dir of this.repoCandidates()) {
        binaries.push(path.join(dir, 'resources', 'bin', exe));
        binaries.push(path.join(dir, 'surfaces', 'allternit-desktop', 'resources', 'bin', exe));
      }
    }
    for (const bin of binaries) {
      if (fs.existsSync(bin)) return { command: bin, args: serveArgs };
    }
    if (this.deps.isPackaged) return null;
    const bun = [this.deps.env.BUN, path.join(this.deps.homedir, '.bun', 'bin', 'bun'), '/opt/homebrew/bin/bun', '/usr/local/bin/bun']
      .find((c): c is string => !!c && fs.existsSync(c));
    if (!bun) return null;
    for (const dir of this.repoCandidates()) {
      const cli = path.join(dir, 'tools', 'system-one-local', 'src', 'cli.ts');
      if (fs.existsSync(cli)) return { command: bun, args: [cli, ...serveArgs] };
    }
    return null;
  }

  resolveLayaScriptsDir(): string | null {
    const candidates = [path.join(this.deps.resourcesPath, 'laya')];
    if (!this.deps.isPackaged) {
      for (const dir of this.repoCandidates()) {
        candidates.push(path.join(dir, 'tools', 'system-one-local', 'laya'));
        candidates.push(path.join(dir, 'resources', 'laya'));
      }
    }
    return (
      candidates.find(
        (c) => fs.existsSync(path.join(c, 'serve-laya.sh')) && fs.existsSync(path.join(c, 'install-laya.sh')),
      ) ?? null
    );
  }

  /** uv from UV, PATH, or the usual install locations (a GUI app's PATH is minimal). */
  resolveUv(): string | null {
    const env = this.deps.env;
    const candidates = [
      env.UV,
      ...(env.PATH ?? '').split(path.delimiter).filter(Boolean).map((d) => path.join(d, 'uv')),
      path.join(this.deps.homedir, '.local', 'bin', 'uv'),
      path.join(this.deps.homedir, '.cargo', 'bin', 'uv'),
      '/opt/homebrew/bin/uv',
      '/usr/local/bin/uv',
    ];
    return candidates.find((c): c is string => !!c && fs.existsSync(c)) ?? null;
  }

  isLayaInstalled(): boolean {
    const venv = path.join(this.root, '.venv');
    if (!fs.existsSync(path.join(venv, 'bin', 'python'))) return false;
    try {
      if (fs.readFileSync(path.join(this.root, 'laya.version'), 'utf8').trim() === LAYA_VERSION) return true;
    } catch {
      // No marker (installed by serve-laya.sh before the marker existed): check the dist-info.
    }
    try {
      return fs
        .readdirSync(path.join(venv, 'lib'))
        .some((py) => fs.existsSync(path.join(venv, 'lib', py, 'site-packages', `laya-${LAYA_VERSION}.dist-info`)));
    } catch {
      return false;
    }
  }

  // ── Health ─────────────────────────────────────────────────────────────────

  async checkSystemOneHealth(): Promise<boolean> {
    try {
      const res = await this.deps.fetch(`${this.s1Url}/healthz`, { signal: AbortSignal.timeout(1000) });
      const body = (await res.json().catch(() => null)) as { ok?: boolean } | null;
      return res.ok && body?.ok === true;
    } catch {
      return false;
    }
  }

  async checkLayaHealth(): Promise<boolean> {
    let healthy = false;
    try {
      const res = await this.deps.fetch(`${this.layaUrl}/health`, { signal: AbortSignal.timeout(1000) });
      healthy = res.ok;
    } catch {
      healthy = false;
    }
    this.layaHealthy = healthy;
    return healthy;
  }

  private async waitFor(check: () => Promise<boolean>, label: string, timeoutMs: number, proc?: () => ChildProcess | null): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (await check()) return;
      if (proc && !proc()) throw new Error(`${label} exited before becoming healthy`);
      await this.deps.sleep(500);
    }
    throw new Error(`${label} did not become healthy within ${Math.round(timeoutMs / 1000)}s`);
  }

  // ── Status / env ───────────────────────────────────────────────────────────

  get backend(): S1Backend {
    return this.layaHealthy ? 'laya_bundled' : 'system_one_local';
  }

  /**
   * Env for the local allternit-api (backend-manager). Explicit env exports win.
   * allternit-api reads it at spawn, so a Laya that turns healthy later applies at
   * the next API start (apiBackend vs backend in getStatus()).
   */
  getApiEnvironment(): Record<string, string> {
    const env = this.deps.env;
    const backend = (env.ALLTERNIT_S1_BACKEND as S1Backend | undefined) || this.backend;
    this.apiBackend = backend;
    const shadowDir = this.shadowDir;
    return {
      ALLTERNIT_S1_URL: env.ALLTERNIT_S1_URL || this.s1Url,
      ALLTERNIT_S1_BACKEND: backend,
      SYSTEM_ONE_LAYA_URL: env.SYSTEM_ONE_LAYA_URL || this.layaUrl,
      ...(shadowDir ? { ALLTERNIT_S1_SHADOW_DIR: shadowDir } : {}),
    };
  }

  async getStatus(): Promise<SystemOneStatus> {
    const [s1Running, layaRunning] = await Promise.all([this.checkSystemOneHealth(), this.checkLayaHealth()]);
    if (!this.needsUv && !this.isLayaInstalled() && this.layaSupported && !this.resolveUv()) this.needsUv = true;
    return {
      layaSupported: this.layaSupported,
      systemOne: {
        available: s1Running || this.resolveSystemOneCommand() !== null,
        running: s1Running,
        url: this.s1Url,
      },
      laya: {
        installed: this.isLayaInstalled(),
        installing: this.installProc !== null,
        running: layaRunning,
        url: this.layaUrl,
        version: LAYA_VERSION,
        checkpoint: this.getCheckpoint(),
        needsUv: this.needsUv,
        ...(this.needsUv ? { uvHint: UV_INSTALL_HINT } : {}),
        installDir: this.root,
      },
      backend: this.backend,
      ...(this.apiBackend ? { apiBackend: this.apiBackend } : {}),
      shadowDir: this.shadowDir,
      ...(this.lastError ? { error: this.lastError } : {}),
    };
  }

  private fail(message: string): void {
    this.lastError = message;
    log.warn(`[SystemOne] ${message}`);
  }

  private emit(progress: SystemOneProgress): void {
    this.deps.emitProgress(progress);
  }

  // ── Install / repair / remove ──────────────────────────────────────────────

  /** Install Laya, or re-run the installer (= repair). Streams progress events. */
  install(): Promise<void> {
    if (this.installPromise) return this.installPromise;
    this.installPromise = this.runInstall().finally(() => {
      this.installPromise = null;
    });
    return this.installPromise;
  }

  private runInstall(): Promise<void> {
    if (!this.layaSupported) return Promise.reject(new Error('Laya is not supported on this platform.'));
    const scriptsDir = this.resolveLayaScriptsDir();
    if (!scriptsDir) return Promise.reject(new Error('Laya install scripts are not available in this build.'));
    const uv = this.resolveUv();
    if (!uv) {
      this.needsUv = true;
      this.emit({ stage: 'needs-uv', message: UV_INSTALL_HINT });
      return Promise.reject(new Error(UV_INSTALL_HINT));
    }
    this.needsUv = false;
    this.lastError = undefined;
    this.emit({ stage: 'starting', message: `Installing Laya ${LAYA_VERSION}…` });

    return new Promise<void>((resolve, reject) => {
      const child = this.deps.spawn('bash', [path.join(scriptsDir, 'install-laya.sh')], {
        env: { ...this.deps.env, LAYA_HOME: this.root, UV: uv },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      this.installProc = child;
      let lastLine = '';
      const onData = (d: Buffer) => {
        for (const line of d.toString().split('\n')) {
          const trimmed = line.trim();
          if (!trimmed) continue;
          lastLine = trimmed;
          this.emit({ stage: 'installing', message: trimmed });
        }
      };
      child.stdout?.on('data', onData);
      child.stderr?.on('data', onData);
      child.on('error', (err) => {
        this.installProc = null;
        this.fail(err.message);
        this.emit({ stage: 'error', message: err.message });
        reject(err);
      });
      child.on('exit', (code, signal) => {
        this.installProc = null;
        if (code === 0) {
          log.info('[SystemOne] Laya install complete');
          this.emit({ stage: 'ready', message: `Laya ${LAYA_VERSION} installed.` });
          resolve();
        } else if (signal === 'SIGTERM') {
          this.emit({ stage: 'cancelled', message: 'Install cancelled.' });
          reject(new Error('Install cancelled.'));
        } else if (code === NEEDS_UV_EXIT) {
          this.needsUv = true;
          this.emit({ stage: 'needs-uv', message: UV_INSTALL_HINT });
          reject(new Error(UV_INSTALL_HINT));
        } else {
          const message = `Laya installer failed (code ${code}): ${lastLine}`;
          this.fail(message);
          this.emit({ stage: 'error', message });
          reject(new Error(message));
        }
      });
    });
  }

  cancelInstall(): boolean {
    if (!this.installProc) return false;
    log.info('[SystemOne] cancelling Laya install');
    this.installProc.kill('SIGTERM');
    return true;
  }

  /** Re-run the installer over the existing venv, then restart Laya. */
  async repair(): Promise<void> {
    this.stopLaya();
    await this.install();
    if (!this.stopped) await this.startLaya();
  }

  /** Stop Laya and delete its managed files (venv, marker, config, log). Never touches the shadow ledger. */
  async remove(): Promise<void> {
    this.stopLaya();
    this.cancelInstall();
    const root = path.resolve(this.root);
    const expected = path.resolve(this.defaultRoot());
    if (root !== expected || !root.startsWith(this.deps.homedir + path.sep)) {
      throw new Error(`Refusing to delete unexpected path: ${root}`);
    }
    log.info(`[SystemOne] removing ${root}`);
    await fs.promises.rm(root, { recursive: true, force: true });
  }

  // ── Lifecycle ──────────────────────────────────────────────────────────────

  /**
   * App start: S1 server now; Laya once installed, installing it first in the
   * background if needed. Never blocks the caller; failures land in status.
   */
  startWithApp(): Promise<void> {
    this.stopped = false;
    const s1 = this.startSystemOne().catch((err) => this.fail(`S1 start failed: ${(err as Error).message}`));
    const laya = (async () => {
      if (!this.layaSupported) return;
      if (!this.isLayaInstalled()) {
        if (!this.resolveUv()) {
          this.needsUv = true;
          log.warn(`[SystemOne] ${UV_INSTALL_HINT}`);
          return;
        }
        await this.install();
      }
      if (!this.stopped) await this.startLaya();
    })().catch((err) => this.fail(`Laya start failed: ${(err as Error).message}`));
    return Promise.all([s1, laya]).then(() => undefined);
  }

  async startSystemOne(): Promise<void> {
    if (await this.checkSystemOneHealth()) return; // Already served (another instance or a dev server).
    if (this.s1Proc) return;
    const cmd = this.resolveSystemOneCommand();
    if (!cmd) throw new Error('System One binary is not available in this build.');
    const shadowDir = this.shadowDir;
    const env: NodeJS.ProcessEnv = {
      ...this.deps.env,
      SYSTEM_ONE_LAYA_URL: this.layaUrl,
      ...(shadowDir ? { ALLTERNIT_S1_SHADOW_DIR: shadowDir } : { SYSTEM_ONE_SHADOW_LOG: '0' }),
    };
    log.info(`[SystemOne] starting S1 on ${this.s1Url}`);
    const child = this.deps.spawnSidecar(cmd.command, cmd.args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
    this.s1Proc = child;
    child.stdout?.on('data', (d: Buffer) => log.info('[SystemOne]', d.toString().trim()));
    child.stderr?.on('data', (d: Buffer) => log.info('[SystemOne]', d.toString().trim()));
    child.on('error', (err) => this.fail(`S1 spawn error: ${err.message}`));
    child.on('exit', (code) => {
      log.warn(`[SystemOne] S1 exited (code ${code})`);
      if (this.s1Proc === child) this.s1Proc = null;
    });
    await this.waitFor(() => this.checkSystemOneHealth(), 'System One', S1_HEALTH_TIMEOUT_MS, () => this.s1Proc);
  }

  async startLaya(): Promise<void> {
    if (!this.layaSupported) throw new Error('Laya is not supported on this platform.');
    if (await this.checkLayaHealth()) return;
    if (this.layaProc) return;
    const scriptsDir = this.resolveLayaScriptsDir();
    if (!scriptsDir) throw new Error('Laya serve script is not available in this build.');
    const cp = this.getCheckpoint();
    const uv = this.resolveUv();
    const env: NodeJS.ProcessEnv = {
      ...this.deps.env,
      LAYA_HOME: this.root,
      LAYA_PORT: String(this.layaPort),
      ...(uv ? { UV: uv } : {}),
      ...(cp.source === 'path' ? { LAYA_CHECKPOINT_PATH: cp.path } : { LAYA_REVISION: cp.revision }),
    };
    if (cp.source !== 'path') delete env.LAYA_CHECKPOINT_PATH;
    log.info(`[SystemOne] starting Laya on ${this.layaUrl} (checkpoint ${cp.path ?? cp.revision})`);
    const child = this.deps.spawnSidecar('bash', [path.join(scriptsDir, 'serve-laya.sh')], {
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.layaProc = child;
    child.stdout?.on('data', (d: Buffer) => log.info('[Laya]', d.toString().trim()));
    child.stderr?.on('data', (d: Buffer) => log.info('[Laya]', d.toString().trim()));
    child.on('error', (err) => this.fail(`Laya spawn error: ${err.message}`));
    child.on('exit', (code) => {
      log.warn(`[SystemOne] Laya exited (code ${code})`);
      if (this.layaProc === child) {
        this.layaProc = null;
        this.layaHealthy = false;
      }
    });
    await this.waitFor(() => this.checkLayaHealth(), 'Laya', LAYA_HEALTH_TIMEOUT_MS, () => this.layaProc);
    log.info('[SystemOne] Laya healthy; S1 backend laya_bundled');
  }

  stopLaya(): void {
    if (this.layaProc) {
      log.info('[SystemOne] stopping Laya');
      this.layaProc.kill('SIGTERM');
      this.layaProc = null;
    }
    this.layaHealthy = false;
  }

  /** App quit: stop what this manager started. Never touches servers it found running. */
  stop(): void {
    this.stopped = true;
    this.cancelInstall();
    this.stopLaya();
    if (this.s1Proc) {
      log.info('[SystemOne] stopping S1');
      this.s1Proc.kill('SIGTERM');
      this.s1Proc = null;
    }
  }
}

export const systemOne = SystemOneManager.getInstance();
