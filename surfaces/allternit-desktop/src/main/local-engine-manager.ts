/**
 * Allternit Local Engine Manager
 *
 * Spawns and manages the local model engine (services/local-engine), a small
 * Rust axum server that serves machine telemetry (CPU/RAM/GPU/disk) and local
 * model runtime lifecycle APIs to the Model Lab web surface on port 3015.
 *
 * Modeled on BackendManager, but intentionally simpler: no API key, no
 * launch config, and startup failures are non-fatal to the desktop app
 * (the caller catches and continues, same posture as the office engine).
 */

import { app } from 'electron';
import { execFileSync, ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import log from 'electron-log';
import { PORTS } from './config.js';
import { spawnSidecar } from './process-lifeline.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

const ENGINE_PORT = PORTS.LOCAL_ENGINE;
const HEALTH_TIMEOUT_MS = 20_000;

export interface LocalEngineStatus {
  installed: boolean;
  running: boolean;
  url: string;
  version?: string;
}

/** One engine pack install, as `GET /engines/packs` and `GET /status` report it. */
export interface EnginePackJob {
  pack_id: string;
  state: 'queued' | 'downloading' | 'verifying' | 'installed' | 'failed';
  version: string | null;
  bytes_done: number;
  bytes_total: number;
  error: string | null;
}

export interface EnginePacksState {
  platform: string;
  dev_feed: boolean;
  installed: Array<{ pack_id: string; engine: string; backend: string; version: string }>;
  jobs: EnginePackJob[];
  started?: string[];
}

const ACTIVE_PACK_STATES = new Set(['queued', 'downloading', 'verifying']);

/** True while any pack is still downloading or being verified. */
export function enginePacksBusy(state: Pick<EnginePacksState, 'jobs'>): boolean {
  return state.jobs.some((j) => ACTIVE_PACK_STATES.has(j.state));
}

/**
 * PIDs listening on `port` from `netstat -ano -p TCP` output (Windows).
 * Rows look like `  TCP    127.0.0.1:3015    0.0.0.0:0    LISTENING    1234`.
 */
export function parseWindowsNetstatListeners(output: string, port: number): number[] {
  const pids = new Set<number>();
  for (const line of output.split(/\r?\n/)) {
    const cols = line.trim().split(/\s+/);
    if (cols.length < 5 || cols[0] !== 'TCP' || cols[3] !== 'LISTENING') continue;
    if (!cols[1].endsWith(`:${port}`)) continue;
    const pid = Number(cols[4]);
    if (Number.isFinite(pid) && pid > 0) pids.add(pid);
  }
  return [...pids];
}

export class LocalEngineManager {
  private static instance: LocalEngineManager;
  private engineProc: ChildProcess | null = null;
  private resolvedBinaryPath: string | null | undefined;
  /**
   * Crash-respawn state. Respawns use capped exponential backoff with jitter
   * so a binary that fails instantly on startup cannot hot-loop the machine.
   * A run that stays up longer than STABLE_RUN_MS resets the attempt count.
   */
  private static readonly BACKOFF_STEPS_MS = [1000, 2000, 5000, 10000, 30000];
  private static readonly STABLE_RUN_MS = 60_000;
  private respawnAttempts = 0;
  private spawnTimestamp = 0;
  private respawnTimer: ReturnType<typeof setTimeout> | null = null;
  /** True while a shutdown was requested — exit events from that kill must not respawn. */
  private intentionalStop = false;
  /** Ring buffer of recent stderr lines for crash diagnostics (startup fail-fast). */
  private stderrLines: string[] = [];

  static getInstance(): LocalEngineManager {
    if (!LocalEngineManager.instance) {
      LocalEngineManager.instance = new LocalEngineManager();
    }
    return LocalEngineManager.instance;
  }

  /** Start the local engine. Returns the base URL. Throws if it cannot start. */
  async ensureStarted(): Promise<string> {
    if (this.engineProc) {
      return this.getUrl();
    }
    if (this.respawnTimer) {
      // A crash respawn is already scheduled; the health-check reuse path
      // below would otherwise race it with a second spawn.
      log.info('[LocalEngine] Respawn already scheduled; skipping duplicate ensureStarted');
      return this.getUrl();
    }

    // Fast path: a single probe that fails with ECONNREFUSED means nothing is
    // (or will be) listening on the port, so skip the patient 20s reuse poll
    // and spawn immediately. Other probe failures (timeouts, resets) may be an
    // engine mid-start, so those fall through to the patient poll to adopt it.
    const existing = await this.probeExistingEngine();
    if (existing === 'ready') {
      log.info(`[LocalEngine] Reusing existing local-engine at ${this.getUrl()}`);
      this.prepareEnginesInBackground();
      return this.getUrl();
    }
    if (existing === 'unknown') {
      try {
        await this.waitForUrl(`${this.getUrl()}/health`, 'existing local-engine');
        log.info(`[LocalEngine] Reusing existing local-engine at ${this.getUrl()}`);
        this.prepareEnginesInBackground();
        return this.getUrl();
      } catch {
        // No healthy engine on the target port; check whether something else
        // occupies the port and reap it before spawning.
      }
    }

    if (this.portListenerPids().length > 0) {
      log.warn(`[LocalEngine] Port ${ENGINE_PORT} is occupied but not healthy; replacing the listener`);
      this.terminateListenerOnPort();
      await new Promise((r) => setTimeout(r, 400));
    }

    let binaryPath = this.resolveBinaryPath();
    let developmentCargoProject: string | null = null;
    if (!binaryPath) {
      const isDev = process.env.NODE_ENV === 'development' || !app.isPackaged;
      if (isDev) {
        // __dirname is surfaces/allternit-desktop/dist/main: four levels up is the repo root.
        const candidate = path.resolve(__dirname, '../../../../services/local-engine');
        if (!fs.existsSync(path.join(candidate, 'Cargo.toml'))) {
          throw new Error(`local-engine is unavailable: no binary and no development Cargo project at ${candidate}`);
        }
        developmentCargoProject = candidate;
        binaryPath = 'cargo';
        log.info(`[LocalEngine] local-engine binary not found; using cargo run from ${candidate}`);
      } else {
        throw new Error(
          'local-engine is not bundled in this install. ' +
          'Build it with `cargo build --release -p allternit-local-engine` and stage it into resources/bin.'
        );
      }
    }

    const env: Record<string, string> = {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([, v]) => v !== undefined) as [string, string][]
      ),
      HOST: '127.0.0.1',
      PORT: String(ENGINE_PORT),
      RUST_LOG: 'info',
    };

    log.info(`[LocalEngine] Starting local-engine on port ${ENGINE_PORT} from ${binaryPath}`);
    const spawned = spawnSidecar(binaryPath, developmentCargoProject ? ['run', '--manifest-path', path.join(developmentCargoProject, 'Cargo.toml')] : [], {
      cwd: developmentCargoProject ?? undefined,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    this.engineProc = spawned;
    this.intentionalStop = false;
    this.spawnTimestamp = Date.now();

    spawned.stdout?.on('data', (d: Buffer) =>
      log.info('[LocalEngine]', d.toString().trim())
    );
    spawned.stderr?.on('data', (d: Buffer) => {
      log.warn('[LocalEngine]', d.toString().trim());
      this.pushStderrLine(d.toString());
    });
    spawned.on('exit', (code) => {
      log.warn(`[LocalEngine] local-engine exited (code ${code})`);
      // A newer spawn (e.g. a manual restart) may already own engineProc; do
      // not clobber its state from this stale exit event.
      if (this.engineProc !== spawned) return;
      this.engineProc = null;

      if (this.intentionalStop) return;

      if (app.isPackaged || process.env.NODE_ENV === 'production') {
        const stableRun = Date.now() - this.spawnTimestamp > LocalEngineManager.STABLE_RUN_MS;
        if (stableRun) this.respawnAttempts = 0;
        const step = Math.min(this.respawnAttempts, LocalEngineManager.BACKOFF_STEPS_MS.length - 1);
        const baseDelay = LocalEngineManager.BACKOFF_STEPS_MS[step];
        // ±20% jitter avoids thundering-herd restarts when many desktops
        // restart an engine at once.
        const delay = Math.round(baseDelay * (0.8 + Math.random() * 0.4));
        this.respawnAttempts += 1;
        log.info(`[LocalEngine] local-engine crashed unexpectedly, respawning in ${delay}ms (attempt ${this.respawnAttempts})...`);
        this.respawnTimer = setTimeout(() => {
          this.respawnTimer = null;
          this.ensureStarted().catch((e) =>
            log.error('[LocalEngine] Failed to auto-restart local-engine:', e)
          );
        }, delay);
        this.respawnTimer.unref?.();
      }
    });

    await this.waitForUrl(`${this.getUrl()}/health`, 'local-engine', spawned);

    log.info(`[LocalEngine] Ready at ${this.getUrl()}`);
    this.prepareEnginesInBackground();
    return this.getUrl();
  }

  /**
   * Inference engines (llama-server builds, the MLX sidecar on Apple silicon)
   * are signed engine packs the local engine downloads and verifies itself;
   * the installer does not bundle them. Ask it to install the packs this
   * machine needs. Resolves as soon as the installs are queued; progress is
   * in `GET /status` (`engine_packs`) and `GET /engines/packs`.
   */
  async ensureEnginePacks(options: { update?: boolean; engines?: string[] } = {}): Promise<EnginePacksState> {
    const res = await fetch(`${this.getUrl()}/engines/packs/ensure`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(options),
      signal: AbortSignal.timeout(15_000),
    });
    const body = (await res.json().catch(() => null)) as (EnginePacksState & { error?: string }) | null;
    if (!res.ok || !body) {
      throw new Error(`engine pack ensure failed (${res.status}): ${body?.error ?? 'no body'}`);
    }
    return body;
  }

  async getEnginePacks(): Promise<EnginePacksState | null> {
    try {
      const res = await fetch(`${this.getUrl()}/engines/packs`, { signal: AbortSignal.timeout(5_000) });
      return res.ok ? ((await res.json()) as EnginePacksState) : null;
    } catch {
      return null;
    }
  }

  /** Wait until no pack is downloading or verifying (logs progress). */
  async waitForEnginePacks(timeoutMs = 10 * 60_000): Promise<EnginePacksState | null> {
    const deadline = Date.now() + timeoutMs;
    let lastLog = 0;
    for (;;) {
      const state = await this.getEnginePacks();
      if (!state || !enginePacksBusy(state)) return state;
      if (Date.now() - lastLog > 5_000) {
        lastLog = Date.now();
        for (const j of state.jobs.filter((x) => ACTIVE_PACK_STATES.has(x.state))) {
          const pct = j.bytes_total ? Math.round((j.bytes_done / j.bytes_total) * 100) : 0;
          log.info(`[LocalEngine] engine pack ${j.pack_id} ${j.version ?? ''}: ${j.state} ${pct}%`);
        }
      }
      if (Date.now() > deadline) return state;
      await new Promise((r) => setTimeout(r, 1_000));
    }
  }

  /**
   * Once per app run, in the background: when this machine already runs
   * local models (a pack is installed or a model is downloaded), make sure
   * the right packs are present and current, so "Run here" does not wait on
   * a download. A machine that never used local models downloads nothing
   * until the UI asks (`POST /engines/packs/ensure`). Never throws.
   */
  private enginePrepStarted = false;

  private prepareEnginesInBackground(): void {
    if (this.enginePrepStarted) return;
    this.enginePrepStarted = true;
    void (async () => {
      try {
        const packs = await this.getEnginePacks();
        if (!packs) {
          log.info('[LocalEngine] engine packs: this engine has no /engines/packs (older build); skipping');
          return;
        }
        const models = await fetch(`${this.getUrl()}/models`, { signal: AbortSignal.timeout(5_000) })
          .then((r) => (r.ok ? r.json() : null))
          .catch(() => null) as { models?: unknown[] } | unknown[] | null;
        const modelCount = Array.isArray(models) ? models.length : Array.isArray(models?.models) ? models.models.length : 0;
        if (packs.installed.length === 0 && modelCount === 0) {
          log.info('[LocalEngine] engine packs: no local models yet; packs install on first Run');
          return;
        }
        const state = await this.ensureEnginePacks({ update: true });
        if (state.started?.length) {
          log.info(`[LocalEngine] engine packs: installing ${state.started.join(', ')}`);
          const done = await this.waitForEnginePacks();
          for (const j of done?.jobs ?? []) {
            if (j.state === 'failed') log.warn(`[LocalEngine] engine pack ${j.pack_id} failed: ${j.error}`);
            else log.info(`[LocalEngine] engine pack ${j.pack_id} ${j.version ?? ''}: ${j.state}`);
          }
        } else {
          log.info(`[LocalEngine] engine packs ready: ${packs.installed.map((p) => `${p.pack_id}@${p.version}`).join(', ') || 'none for this machine'}`);
        }
      } catch (err) {
        log.warn('[LocalEngine] engine pack check failed:', err);
      }
    })();
  }

  async stop(): Promise<void> {
    if (this.respawnTimer) {
      clearTimeout(this.respawnTimer);
      this.respawnTimer = null;
    }
    this.intentionalStop = true;
    this.respawnAttempts = 0;
    if (this.engineProc) {
      log.info('[LocalEngine] Stopping local-engine…');
      this.engineProc.kill('SIGTERM');
      this.engineProc = null;
    }
  }

  getUrl(): string {
    return `http://127.0.0.1:${ENGINE_PORT}`;
  }

  async getStatus(): Promise<LocalEngineStatus> {
    const binaryPath = this.resolveBinaryPath(false);
    const installed = binaryPath !== null;
    let running = false;
    let version: string | undefined;

    try {
      const res = await fetch(`${this.getUrl()}/health`, {
        signal: AbortSignal.timeout(1000),
      });
      const body = await res.clone().json().catch(() => null) as { version?: string; status?: string } | null;
      if (res.ok) {
        running = true;
        version = body?.version;
      }
    } catch {
      running = false;
    }

    return { installed, running, url: this.getUrl(), version };
  }

  /** PIDs (excluding ours) listening on the engine port, via lsof (netstat on Windows). */
  private portListenerPids(): number[] {
    if (process.platform === 'win32') {
      try {
        const out = execFileSync('netstat', ['-ano', '-p', 'TCP'], { encoding: 'utf8', windowsHide: true });
        return parseWindowsNetstatListeners(out, ENGINE_PORT).filter((pid) => pid !== process.pid);
      } catch {
        return [];
      }
    }
    try {
      const out = execFileSync(
        'lsof',
        ['-nP', `-iTCP:${ENGINE_PORT}`, '-sTCP:LISTEN', '-t'],
        { encoding: 'utf8' },
      );
      return out
        .trim()
        .split(/\s+/)
        .filter(Boolean)
        .map((pidText) => Number(pidText))
        .filter((pid) => Number.isFinite(pid) && pid > 0 && pid !== process.pid);
    } catch {
      // lsof missing or nothing listening
      return [];
    }
  }

  /** SIGTERM whatever is listening on the engine port. Packaged Desktop owns :3015. */
  private terminateListenerOnPort(): void {
    for (const pid of this.portListenerPids()) {
      log.warn(`[LocalEngine] Stopping leftover listener pid ${pid} on :${ENGINE_PORT}`);
      try {
        process.kill(pid, 'SIGTERM');
      } catch (error) {
        log.warn(`[LocalEngine] Could not stop pid ${pid}:`, error);
      }
    }
  }

  /**
   * Classify whatever is on the engine port with a single short probe.
   * - 'ready': an existing healthy engine answers /health — reuse it.
   * - 'none': ECONNREFUSED — nothing is listening, spawn immediately.
   * - 'unknown': any other failure (timeout/reset — possibly an engine
   *   mid-start) — caller falls through to the patient waitForUrl poll.
   */
  private async probeExistingEngine(): Promise<'ready' | 'none' | 'unknown'> {
    try {
      const res = await fetch(`${this.getUrl()}/health`, {
        signal: AbortSignal.timeout(800),
      });
      return res.ok ? 'ready' : 'unknown';
    } catch (probeErr) {
      if ((probeErr as { cause?: { code?: string } })?.cause?.code === 'ECONNREFUSED') {
        return 'none';
      }
      return 'unknown';
    }
  }

  /**
   * Poll a health URL until it answers OK. When a freshly spawned child is
   * provided, its exit fails the wait immediately instead of burning the full
   * HEALTH_TIMEOUT_MS against a process that already crashed, and the exit
   * listener is removed as soon as health wins.
   */
  private async waitForUrl(url: string, label: string, child?: ChildProcess): Promise<void> {
    const deadline = Date.now() + HEALTH_TIMEOUT_MS;
    const crashState = { crashed: false, code: null as number | null };
    const onExit = (code: number | null) => {
      crashState.crashed = true;
      crashState.code = code;
    };
    if (child) child.once('exit', onExit);

    try {
      while (Date.now() < deadline) {
        if (crashState.crashed) {
          throw new Error(
            `${label} exited during startup (code ${crashState.code}): ${this.recentStderr().join(' | ')}`,
          );
        }
        try {
          const res = await fetch(url, { signal: AbortSignal.timeout(500) });
          if (res.ok) {
            return;
          }
        } catch {
          // Not ready yet.
        }
        await new Promise((r) => setTimeout(r, 200));
      }

      throw new Error(`${label} did not start within ${HEALTH_TIMEOUT_MS / 1000}s`);
    } finally {
      if (child) child.removeListener('exit', onExit);
    }
  }

  private pushStderrLine(text: string): void {
    for (const line of text.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      this.stderrLines.push(trimmed);
      if (this.stderrLines.length > 20) this.stderrLines.shift();
    }
  }

  private recentStderr(): string[] {
    return this.stderrLines.slice(-5);
  }

  private resolveBinaryPath(logDiscovery = true): string | null {
    if (this.resolvedBinaryPath !== undefined) {
      return this.resolvedBinaryPath;
    }

    // Cargo names the crate's bin `local-engine` (see [[bin]] in
    // services/local-engine/Cargo.toml); packaging stages it as
    // allternit-local-engine for a consistent resources/bin layout.
    const packagedName = process.platform === 'win32' ? 'allternit-local-engine.exe' : 'allternit-local-engine';
    const devName = process.platform === 'win32' ? 'local-engine.exe' : 'local-engine';

    // __dirname is dist/main; go up four levels to reach the repo root.
    const repoRoot = path.resolve(__dirname, '..', '..', '..', '..');
    const candidates = app.isPackaged
      ? [
          path.join(process.resourcesPath ?? '', 'bin', packagedName),
        ]
      : [
          path.join(repoRoot, 'target', 'debug', devName),
          path.join(repoRoot, 'target', 'release', devName),
          path.join(repoRoot, 'target', 'debug', packagedName),
          path.join(repoRoot, 'target', 'release', packagedName),
          path.join(process.resourcesPath ?? '', 'bin', packagedName),
          path.join(__dirname, '..', '..', 'resources', 'bin', packagedName),
        ];

    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) {
        this.resolvedBinaryPath = candidate;
        if (logDiscovery) {
          log.info(`[LocalEngine] Found binary at: ${candidate}`);
        }
        return candidate;
      }
    }

    log.error('[LocalEngine] local-engine binary not found. Searched:', candidates);
    this.resolvedBinaryPath = null;
    return null;
  }
}

export const localEngineManager = LocalEngineManager.getInstance();
