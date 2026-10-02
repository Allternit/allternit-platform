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
 * getApiEnvironment(): ALLTERNIT_S1_URL; ALLTERNIT_S1_BACKEND only when exported
 * explicitly. Otherwise allternit-api sends "auto" and the S1 server uses Laya
 * while it is healthy (#1113), so a Laya that turns healthy later needs no restart.
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
/** Local embeddings for the memory index (serve-embed.sh, shares Laya's venv). */
export const EMBED_DEFAULT_PORT = 7719;
export const LAYA_VERSION = '0.3.22';
/** Q29: convaiinnovations/laya, typed-decisions, rev 55cf4c4. */
export const LAYA_PINNED_REVISION = '55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851';
export const UV_INSTALL_HINT =
  "Laya needs uv. Install it with 'brew install uv' or 'curl -LsSf https://astral.sh/uv/install.sh | sh', then choose Repair.";

const S1_HEALTH_TIMEOUT_MS = 30_000;
/** First Laya start downloads ~600 MB of weights. */
const LAYA_HEALTH_TIMEOUT_MS = 15 * 60_000;
const NEEDS_UV_EXIT = 3;
/** Q26: in live mode the first canary sync goes out ~60 s after S1 starts, then every 15 min. */
const CANARY_SYNC_FIRST_DELAY_MS = 60_000;
const CANARY_SYNC_INTERVAL_MS = 15 * 60_000;
/** A canary sync that hangs is killed instead of delaying the next tick. */
const CANARY_SYNC_TIMEOUT_MS = 60_000;

export type S1Backend = 'laya_bundled' | 'system_one_local' | 'auto';

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
  /** Q28 opt-in: the ledger also keeps the raw decision state (fine-tuning text). */
  shadowState: boolean;
  /** Q26 live mode: S1 consults the canary (ALLTERNIT_S1_MODE=live on the S1 process). */
  liveMode: boolean;
  /** Local embeddings for the memory index (serve-embed.sh). */
  embed: { running: boolean; url: string };
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
  private embedProc: ChildProcess | null = null;
  private installProc: ChildProcess | null = null;
  private installPromise: Promise<void> | null = null;
  private layaHealthy = false;
  private needsUv = false;
  private lastError: string | undefined;
  private apiBackend: S1Backend | undefined;
  private stopped = false;
  private depsCache: SystemOneDeps | undefined;
  private canarySyncTimer: ReturnType<typeof setTimeout> | null = null;
  private canarySyncRunning = false;

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

  get embedPort(): number {
    return Number(this.deps.env.ALLTERNIT_EMBED_PORT) || EMBED_DEFAULT_PORT;
  }

  get embedUrl(): string {
    return `http://127.0.0.1:${this.embedPort}`;
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
   * S1's own settings, beside the shadow ledger, so removing Laya (which
   * deletes its whole root) never drops them.
   */
  private get settingsPath(): string {
    return path.join(this.deps.homedir, '.allternit', 'system-one', 'settings.json');
  }

  private readSettings(): Record<string, unknown> {
    try {
      const parsed = JSON.parse(fs.readFileSync(this.settingsPath, 'utf8'));
      return parsed && typeof parsed === 'object' ? parsed : {};
    } catch {
      return {};
    }
  }

  /**
   * Q28 opt-in: also keep the raw decision state (the fine-tuning text) in
   * the shadow ledger. The saved setting wins; without one, the legacy
   * SYSTEM_ONE_SHADOW_STATE=1 export still counts. Off by default.
   */
  get shadowState(): boolean {
    const saved = this.readSettings().shadowState;
    if (typeof saved === 'boolean') return saved;
    return this.deps.env.SYSTEM_ONE_SHADOW_STATE === '1';
  }

  /** Save the raw-state opt-in and restart the S1 server this manager owns so it applies. */
  async setShadowState(enabled: boolean): Promise<boolean> {
    const settings = this.readSettings();
    settings.shadowState = enabled;
    await fs.promises.mkdir(path.dirname(this.settingsPath), { recursive: true });
    await fs.promises.writeFile(this.settingsPath, `${JSON.stringify(settings, null, 2)}\n`);
    log.info(`[SystemOne] raw decision state in the shadow ledger: ${enabled ? 'on' : 'off'}`);
    await this.restartSystemOne();
    return this.shadowState;
  }

  /**
   * Q26 live mode: S1 only consults the canary when its process env has
   * ALLTERNIT_S1_MODE=live (tools/system-one-local/src/server.ts). The saved
   * setting wins; without one, the ALLTERNIT_S1_MODE export still counts.
   * Off by default.
   */
  get liveMode(): boolean {
    const saved = this.readSettings().liveMode;
    if (typeof saved === 'boolean') return saved;
    return this.deps.env.ALLTERNIT_S1_MODE === 'live';
  }

  /** Save the live-mode switch and restart the S1 server this manager owns so it applies. */
  async setLiveMode(enabled: boolean): Promise<boolean> {
    const settings = this.readSettings();
    settings.liveMode = enabled;
    await fs.promises.mkdir(path.dirname(this.settingsPath), { recursive: true });
    await fs.promises.writeFile(this.settingsPath, `${JSON.stringify(settings, null, 2)}\n`);
    log.info(`[SystemOne] live mode (canary consulted per decision): ${enabled ? 'on' : 'off'}`);
    await this.restartSystemOne();
    return this.liveMode;
  }

  /** Restart the owned S1 server so env-affecting settings apply (shared by the setters above). */
  private async restartSystemOne(): Promise<void> {
    if (!this.s1Proc) return;
    const proc = this.s1Proc;
    this.s1Proc = null;
    this.clearCanarySyncTimer();
    proc.kill('SIGTERM');
    await new Promise<void>((resolve) => {
      if (proc.exitCode != null || proc.signalCode != null) return resolve();
      proc.once('exit', () => resolve());
      setTimeout(resolve, 5000);
    });
    await this.startSystemOne().catch((err) => this.fail(`S1 restart failed: ${(err as Error).message}`));
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

  async checkEmbedHealth(): Promise<boolean> {
    try {
      const res = await this.deps.fetch(`${this.embedUrl}/health`, { signal: AbortSignal.timeout(1000) });
      return res.ok;
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
   * Without an explicit ALLTERNIT_S1_BACKEND the API uses "auto": the S1 server
   * picks Laya per decision while it is healthy, so no API restart is needed.
   */
  getApiEnvironment(): Record<string, string> {
    const env = this.deps.env;
    const explicit = env.ALLTERNIT_S1_BACKEND as S1Backend | undefined;
    this.apiBackend = explicit || 'auto';
    const shadowDir = this.shadowDir;
    return {
      ALLTERNIT_S1_URL: env.ALLTERNIT_S1_URL || this.s1Url,
      ...(explicit ? { ALLTERNIT_S1_BACKEND: explicit } : {}),
      SYSTEM_ONE_LAYA_URL: env.SYSTEM_ONE_LAYA_URL || this.layaUrl,
      ...(shadowDir ? { ALLTERNIT_S1_SHADOW_DIR: shadowDir } : {}),
      // The memory index embeds through the local sidecar this manager runs
      // (Laya's platforms only); an explicit export (or "off") wins.
      ...(env.ALLTERNIT_EMBED_URL
        ? { ALLTERNIT_EMBED_URL: env.ALLTERNIT_EMBED_URL }
        : this.layaSupported
          ? { ALLTERNIT_EMBED_URL: this.embedUrl }
          : {}),
    };
  }

  async getStatus(): Promise<SystemOneStatus> {
    const [s1Running, layaRunning, embedRunning] = await Promise.all([
      this.checkSystemOneHealth(),
      this.checkLayaHealth(),
      this.checkEmbedHealth(),
    ]);
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
      shadowState: this.shadowState,
      liveMode: this.liveMode,
      embed: { running: embedRunning, url: this.deps.env.ALLTERNIT_EMBED_URL || this.embedUrl },
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
    this.stopEmbed();
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
      // Embeddings ride on Laya's venv; a failure only degrades memory search.
      if (!this.stopped) await this.startEmbed().catch((err) => this.fail(`Embedding server start failed: ${(err as Error).message}`));
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
    if (this.shadowState) env.SYSTEM_ONE_SHADOW_STATE = '1';
    else delete env.SYSTEM_ONE_SHADOW_STATE;
    if (this.liveMode) env.ALLTERNIT_S1_MODE = 'live';
    else delete env.ALLTERNIT_S1_MODE;
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
    this.scheduleCanarySync(CANARY_SYNC_FIRST_DELAY_MS);
  }

  // ── Q26 canary sync (live mode only) ───────────────────────────────────────

  private clearCanarySyncTimer(): void {
    if (this.canarySyncTimer) {
      clearTimeout(this.canarySyncTimer);
      this.canarySyncTimer = null;
    }
  }

  /** Arm the next `canary sync`; a no-op unless live mode is on and the owned S1 is running. */
  private scheduleCanarySync(delayMs: number): void {
    this.clearCanarySyncTimer();
    if (this.stopped || !this.liveMode || !this.s1Proc) return;
    this.canarySyncTimer = setTimeout(() => {
      this.canarySyncTimer = null;
      void this.runCanarySync().finally(() => {
        if (!this.stopped && this.liveMode && this.s1Proc) this.scheduleCanarySync(CANARY_SYNC_INTERVAL_MS);
      });
    }, delayMs);
    this.canarySyncTimer.unref?.();
  }

  /**
   * Feed audited shadow-ledger outcomes to the canary (CUSUM auto-rollback
   * never sees them otherwise). Same CLI `resolveSystemOneCommand()` finds for
   * `serve`, with the serve args swapped for `canary sync`. Never throws.
   */
  private async runCanarySync(): Promise<void> {
    if (this.canarySyncRunning) return; // a previous sync is still running: skip this tick
    if (this.stopped || !this.liveMode || !this.s1Proc) return;
    const cmd = this.resolveSystemOneCommand();
    if (!cmd) return;
    const serveArgs = ['serve', '--port', String(this.s1Port)];
    const cliPrefix = cmd.args.slice(0, cmd.args.length - serveArgs.length);
    const env: NodeJS.ProcessEnv = { ...this.deps.env };
    const shadowDir = this.shadowDir;
    if (shadowDir) env.ALLTERNIT_S1_SHADOW_DIR = shadowDir;
    this.canarySyncRunning = true;
    try {
      log.info('[SystemOne] syncing the S1 canary with the shadow ledger');
      await new Promise<void>((resolve) => {
        const child = this.deps.spawnSidecar(cmd.command, [...cliPrefix, 'canary', 'sync'], {
          env,
          timeout: CANARY_SYNC_TIMEOUT_MS,
          stdio: ['ignore', 'pipe', 'pipe'],
        });
        child.stdout?.on('data', (d: Buffer) => log.info('[SystemOne] canary sync:', d.toString().trim()));
        child.stderr?.on('data', (d: Buffer) => log.info('[SystemOne] canary sync:', d.toString().trim()));
        child.once('exit', () => resolve());
        child.once('error', () => resolve());
      });
    } catch (err) {
      log.warn(`[SystemOne] canary sync failed: ${(err as Error).message}`);
    } finally {
      this.canarySyncRunning = false;
    }
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

  /** Serve local embeddings (serve-embed.sh) from Laya's venv on 7719. */
  async startEmbed(): Promise<void> {
    if (!this.layaSupported) return;
    if (this.deps.env.ALLTERNIT_EMBED_URL) return; // pointed elsewhere (or "off")
    if (await this.checkEmbedHealth()) return;
    if (this.embedProc) return;
    const scriptsDir = this.resolveLayaScriptsDir();
    if (!scriptsDir || !fs.existsSync(path.join(scriptsDir, 'serve-embed.sh'))) {
      throw new Error('the embedding server script is not available in this build');
    }
    const uv = this.resolveUv();
    const env: NodeJS.ProcessEnv = {
      ...this.deps.env,
      LAYA_HOME: this.root,
      EMBED_PORT: String(this.embedPort),
      ...(uv ? { PATH: [path.dirname(uv), this.deps.env.PATH ?? ''].filter(Boolean).join(path.delimiter) } : {}),
    };
    log.info(`[SystemOne] starting the embedding server on ${this.embedUrl}`);
    const child = this.deps.spawnSidecar('bash', [path.join(scriptsDir, 'serve-embed.sh')], {
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.embedProc = child;
    child.stdout?.on('data', (d: Buffer) => log.info('[Embed]', d.toString().trim()));
    child.stderr?.on('data', (d: Buffer) => log.info('[Embed]', d.toString().trim()));
    child.on('error', (err) => this.fail(`Embedding server spawn error: ${err.message}`));
    child.on('exit', (code) => {
      log.warn(`[SystemOne] embedding server exited (code ${code})`);
      if (this.embedProc === child) this.embedProc = null;
    });
    await this.waitFor(() => this.checkEmbedHealth(), 'Embedding server', LAYA_HEALTH_TIMEOUT_MS, () => this.embedProc);
    log.info('[SystemOne] embedding server healthy');
  }

  stopEmbed(): void {
    if (this.embedProc) {
      log.info('[SystemOne] stopping the embedding server');
      this.embedProc.kill('SIGTERM');
      this.embedProc = null;
    }
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
    this.stopEmbed();
    this.stopLaya();
    this.clearCanarySyncTimer();
    if (this.s1Proc) {
      log.info('[SystemOne] stopping S1');
      this.s1Proc.kill('SIGTERM');
      this.s1Proc = null;
    }
  }
}

export const systemOne = SystemOneManager.getInstance();
