/**
 * Connector Sidecar Manager
 *
 * Manages the open-connector process — the self-hosted OAuth/API-key
 * connector gateway (see services/open-connector/PROVENANCE.md) that
 * allternit-api's connector routes proxy to
 * (cmd/allternit-api/src/open_connector_proxy.rs) and that gizzi-code's
 * Lens vault connectors call directly for providers the curated Rust path
 * can't serve correctly yet (see cmd/gizzi-code/src/vault/connectors/sidecar.ts).
 *
 * Production shape (PR #245 follow-up to the PR #244 crash-loop fix):
 *   - The sidecar ships as a single esbuild bundle
 *     (resources/connector-sidecar/dist/server.mjs, built by
 *     scripts/prepare-connector-sidecar.cjs) — no src/ tree, no
 *     node_modules copy in the app.
 *   - It binds an EPHEMERAL loopback port (PORT=0) and announces the real
 *     port on stdout; this manager probes /health on that port before
 *     declaring readiness. Nothing races a fixed port, and a dev server or
 *     old LaunchAgent on 8014 can no longer wedge the app into an
 *     EADDRINUSE crash loop.
 *   - Crashes are supervised with exponential backoff and a bounded number
 *     of restarts; after that the sidecar is DEGRADED (not silently
 *     respawned forever) and the failure is surfaced via onDegraded so the
 *     shell can show it in service state.
 *
 * Runtime is Electron's own bundled Node (ELECTRON_RUN_AS_NODE=1), the
 * same trick as before — open-connector uses node:sqlite, which only
 * Node's built-in module provides.
 */

import { app } from 'electron';
import { spawn, ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import log from 'electron-log';
import { URLS } from './config.js';

const ANNOUNCE_TIMEOUT_MS = 25_000;
const HEALTH_TIMEOUT_MS = 15_000;
const MAX_CRASH_RESTARTS = 5;
// 1s, 2s, 4s, 8s, 16s — then give up and go degraded.
const BACKOFF_DELAYS_MS = [1000, 2000, 4000, 8000, 16000];

const ANNOUNCE_RE = /\{.*"allternitAnnounce"\s*:\s*"listening".*"port"\s*:\s*(\d+).*\}/;

export type ConnectorSidecarStatus = 'stopped' | 'starting' | 'running' | 'degraded';

export interface ConnectorSidecarStartConfig {
  /** ALLTERNIT_ENCRYPTION_KEY-style shared secret — reused here so the
   * sidecar's own credential store (services/open-connector's
   * OOMOL_CONNECT_ENCRYPTION_KEY) is encrypted with the same
   * authManager-persisted key as everything else, not a second one. */
  encryptionKey: string;
  adminToken: string;
  runtimeToken: string;
}

export class ConnectorSidecarManager {
  private static instance: ConnectorSidecarManager;
  private proc: ChildProcess | null = null;
  private stopping = false;
  private lastConfig: ConnectorSidecarStartConfig | null = null;
  private resolvedEntryPath: string | null | undefined;
  private becameReady = false;
  private boundUrl: string | null = null;
  private status: ConnectorSidecarStatus = 'stopped';
  private crashRestarts = 0;
  private degradedListener: (() => void) | null = null;

  static getInstance(): ConnectorSidecarManager {
    if (!ConnectorSidecarManager.instance) {
      ConnectorSidecarManager.instance = new ConnectorSidecarManager();
    }
    return ConnectorSidecarManager.instance;
  }

  /**
   * Called when the sidecar exhausts its crash budget. The shell uses this
   * to flip service state to degraded instead of leaving a silent hole.
   */
  onDegraded(listener: (() => void) | null): void {
    this.degradedListener = listener;
  }

  getStatus(): ConnectorSidecarStatus {
    return this.status;
  }

  /** Start the connector sidecar. Returns its base URL. Idempotent. */
  async start(config: ConnectorSidecarStartConfig): Promise<string> {
    this.lastConfig = config;
    if (this.proc) {
      return this.getUrl();
    }

    const entryPath = this.resolveEntryPath();
    if (!entryPath) {
      log.warn(
        '[ConnectorSidecarManager] connector sidecar bundle not found — connector-backed ' +
          'Lens sources (GitHub, Notion, Linear, etc via the sidecar) will be unavailable. ' +
          'Run the desktop build pipeline (npm run prepare:connector-sidecar).'
      );
      this.status = 'degraded';
      this.degradedListener?.();
      throw new Error('connector sidecar bundle not found');
    }

    const sidecarRoot = resolveSidecarRoot(entryPath);
    const dataDir = path.join(app.getPath('userData'), 'connector-sidecar-data');
    const catalogDir = path.join(sidecarRoot, 'catalog', 'apps');
    const migrationsDir = path.join(sidecarRoot, 'migrations');
    fs.mkdirSync(dataDir, { recursive: true });

    const env: Record<string, string> = {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([, v]) => v !== undefined) as [string, string][]
      ),
      ELECTRON_RUN_AS_NODE: '1',
      // Production logging shape: pino only skips its pino-pretty worker
      // transport when NODE_ENV=production, and the pretty transport is a
      // devDependency that does not exist in the bundle.
      NODE_ENV: 'production',
      // Ephemeral port — the real one comes back via the stdout announce.
      PORT: '0',
      HOST: '127.0.0.1',
      OOMOL_CONNECT_ANNOUNCE_PORT: '1',
      OOMOL_CONNECT_ORIGIN: URLS.API,
      OOMOL_CONNECT_DATA_DIR: dataDir,
      OOMOL_CONNECT_CATALOG_DIR: catalogDir,
      OOMOL_CONNECT_MIGRATIONS_DIR: migrationsDir,
      OOMOL_CONNECT_ENCRYPTION_KEY: config.encryptionKey,
      OOMOL_CONNECT_ADMIN_TOKEN: config.adminToken,
      OOMOL_CONNECT_RUNTIME_TOKEN: config.runtimeToken,
    };

    log.info(`[ConnectorSidecarManager] Starting open-connector (ephemeral port) from ${entryPath}`);
    this.status = 'starting';

    const proc = spawn(process.execPath, [entryPath], {
      env,
      cwd: sidecarRoot,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    this.proc = proc;

    proc.on('exit', (code) => {
      const intentionalStop = this.stopping;
      this.stopping = false;
      if (this.proc === proc) this.proc = null;
      if (intentionalStop) return;
      this.boundUrl = null;
      log.warn(`[ConnectorSidecarManager] exited (code ${code})`);
      if (this.becameReady && this.lastConfig) {
        // Crash after a healthy start: supervise with bounded backoff.
        this.scheduleCrashRestart();
      }
      // A death before readiness is surfaced by the announce promise's own
      // exit handler, which rejects the in-flight start().
    });

    let stderrTail = '';
    proc.stderr?.on('data', (d: Buffer) => {
      stderrTail = (stderrTail + d.toString()).slice(-4000);
      log.warn('[ConnectorSidecar]', d.toString().trim());
    });

    const announcePort = await new Promise<number | null>((resolve, reject) => {
      let stdoutBuf = '';
      const timer = setTimeout(() => {
        resolve(null);
      }, ANNOUNCE_TIMEOUT_MS);
      proc.once('error', (error) => {
        clearTimeout(timer);
        reject(error);
      });
      proc.stdout?.on('data', (d: Buffer) => {
        const text = d.toString();
        const match = ANNOUNCE_RE.exec(text);
        if (match) {
          clearTimeout(timer);
          resolve(Number(match[1]));
          return;
        }
        stdoutBuf = (stdoutBuf + text).slice(-4000);
        // Buffered lines can split across chunks; also try the whole buffer.
        const bufMatch = ANNOUNCE_RE.exec(stdoutBuf);
        if (bufMatch) {
          clearTimeout(timer);
          resolve(Number(bufMatch[1]));
          return;
        }
        for (const line of text.split('\n')) {
          if (line.trim()) log.info('[ConnectorSidecar]', line.trim());
        }
      });
      proc.once('exit', (code) => {
        clearTimeout(timer);
        reject(
          new Error(
            `connector sidecar exited (code ${code}) before announcing its port` +
              (stderrTail ? `: ${stderrTail.trim().split('\n').slice(-3).join(' | ')}` : '')
          )
        );
      });
    }).catch((error) => {
      if (this.proc === proc) this.proc = null;
      this.status = 'degraded';
      this.degradedListener?.();
      throw error;
    });

    if (announcePort === null) {
      proc.kill('SIGTERM');
      if (this.proc === proc) this.proc = null;
      this.status = 'degraded';
      this.degradedListener?.();
      throw new Error(
        `connector sidecar did not announce its port within ${ANNOUNCE_TIMEOUT_MS / 1000}s` +
          (stderrTail ? `: ${stderrTail.trim().split('\n').slice(-3).join(' | ')}` : '')
      );
    }

    this.boundUrl = `http://127.0.0.1:${announcePort}`;
    proc.stdout?.removeAllListeners('data');
    proc.stdout?.on('data', (d: Buffer) => log.info('[ConnectorSidecar]', d.toString().trim()));

    await this.waitUntilReady();
    this.becameReady = true;
    this.status = 'running';
    this.crashRestarts = 0;
    log.info(`[ConnectorSidecarManager] Ready at ${this.getUrl()}`);
    return this.getUrl();

    function resolveSidecarRoot(entry: string): string {
      // Packaged: <resources>/connector-sidecar/dist/server.mjs
      // Dev fallback: services/open-connector/src/server/index.ts
      return entry.endsWith(path.join('dist', 'server.mjs'))
        ? path.dirname(path.dirname(entry))
        : path.dirname(path.dirname(path.dirname(entry)));
    }
  }

  stop(): void {
    this.lastConfig = null;
    if (this.proc) {
      log.info('[ConnectorSidecarManager] Stopping…');
      this.stopping = true;
      this.proc.kill('SIGTERM');
      this.proc = null;
    }
    this.boundUrl = null;
    this.becameReady = false;
    this.status = 'stopped';
  }

  /** Manual retry after degraded — resets the crash budget and restarts. */
  async restart(): Promise<string> {
    this.crashRestarts = 0;
    if (!this.lastConfig) {
      throw new Error('connector sidecar was never configured');
    }
    return this.start(this.lastConfig);
  }

  /** Base URL of the running sidecar. Falls back to the default loopback
   * URL when never started, so consumers that read it early (gizzi/API env)
   * get a stable shape. */
  getUrl(): string {
    return this.boundUrl ?? URLS.CONNECTOR_SIDECAR;
  }

  isRunning(): boolean {
    return this.proc !== null;
  }

  private scheduleCrashRestart(): void {
    if (
      !this.lastConfig ||
      this.crashRestarts >= MAX_CRASH_RESTARTS ||
      !(app.isPackaged || process.env.NODE_ENV === 'production')
    ) {
      this.status = 'degraded';
      log.error(
        `[ConnectorSidecarManager] Sidecar crashed ${this.crashRestarts} time(s) and will not be ` +
          'respawned — connector-backed sources are unavailable until the app restarts or the ' +
          'sidecar is restarted manually.'
      );
      this.degradedListener?.();
      return;
    }
    const delay = BACKOFF_DELAYS_MS[Math.min(this.crashRestarts, BACKOFF_DELAYS_MS.length - 1)];
    this.crashRestarts += 1;
    log.info(`[ConnectorSidecarManager] Connector sidecar crashed unexpectedly, respawning in ${delay / 1000}s…`);
    setTimeout(() => {
      if (this.lastConfig) {
        this.start(this.lastConfig).catch((e) =>
          log.error('[ConnectorSidecarManager] Failed to auto-restart connector sidecar:', e)
        );
      }
    }, delay);
  }

  private async waitUntilReady(): Promise<void> {
    const deadline = Date.now() + HEALTH_TIMEOUT_MS;
    const url = `${this.getUrl()}/health`;
    while (Date.now() < deadline) {
      if (!this.proc) {
        throw new Error('connector sidecar exited before becoming ready');
      }
      try {
        const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
        if (res.ok) return;
      } catch {
        // Not ready yet
      }
      await new Promise((r) => setTimeout(r, 200));
    }
    throw new Error(`connector sidecar did not become healthy within ${HEALTH_TIMEOUT_MS / 1000}s`);
  }

  private resolveEntryPath(): string | null {
    if (this.resolvedEntryPath !== undefined) {
      return this.resolvedEntryPath;
    }

    const packagedBundle = path.join(process.resourcesPath ?? '', 'connector-sidecar', 'dist', 'server.mjs');
    const candidates = app.isPackaged
      ? [packagedBundle]
      : [
          // Dev monorepo: prefer the staged bundle when the build pipeline
          // has run it; fall back to running the TS source directly.
          path.join(app.getAppPath(), 'resources', 'connector-sidecar', 'dist', 'server.mjs'),
          path.join(app.getAppPath(), '..', '..', 'services', 'open-connector', 'src', 'server', 'index.ts'),
        ];

    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) {
        this.resolvedEntryPath = candidate;
        return candidate;
      }
    }
    this.resolvedEntryPath = null;
    return null;
  }
}

export const connectorSidecarManager = ConnectorSidecarManager.getInstance();
