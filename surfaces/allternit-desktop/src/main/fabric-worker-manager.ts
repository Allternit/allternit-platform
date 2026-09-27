/**
 * Fabric Transport Worker Manager (managed runtime, consumer desktop P1)
 *
 * The desktop main process owns the fabric-transport worker lifecycle:
 *   - spawn: the bundled gizzi-code binary as `gizzi-code fabric-worker`
 *     (the daemon entry ships inside the single-file sidecar), with
 *     `ALLTERNIT_GIZZI_TOKEN` from the Keychain-backed secure store;
 *   - readiness: the daemon logs a structured `worker.daemon_start` JSON
 *     line on stdout — first line marks the worker up;
 *   - crash respawn: exponential backoff with ±20% jitter, mirroring
 *     BackendManager's ping-pong;
 *   - graceful quit: SIGTERM on app exit — the worker finishes its in-flight
 *     claim and exits 0 (abandoned leases requeue via the control-plane
 *     sweeper).
 *
 * The claim protocol itself is unchanged; this manager only owns process
 * lifecycle, exactly like BackendManager owns allternit-api.
 */

import { spawn, ChildProcess } from 'child_process';
import { app } from 'electron';
import * as fs from 'fs';
import * as path from 'path';
import log from 'electron-log';
import { URLS } from './config.js';

export type FabricWorkerStatus = 'stopped' | 'starting' | 'up' | 'down';

export interface FabricWorkerState {
  status: FabricWorkerStatus;
  detail: string;
}

const BACKOFF_STEPS_MS = [1_000, 2_000, 5_000, 10_000, 15_000, 30_000];
/** A run shorter than this that exits non-zero is a crash worth respawning. */
const STABLE_RUN_MS = 5_000;
const READINESS_LINE = '"event":"worker.daemon_start"';

export class FabricWorkerManager {
  private proc: ChildProcess | null = null;
  private spawnTimestamp = 0;
  private respawnAttempts = 0;
  private respawnTimer: NodeJS.Timeout | null = null;
  private intentionalStop = false;
  private status: FabricWorkerState = { status: 'stopped', detail: 'Not started' };
  private listeners = new Set<(state: FabricWorkerState) => void>();

  onStateChange(listener: (state: FabricWorkerState) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  getStatus(): FabricWorkerState {
    return this.status;
  }

  private setStatus(status: FabricWorkerStatus, detail: string): void {
    this.status = { status, detail };
    for (const listener of this.listeners) listener(this.status);
  }

  private resolveBinaryPath(): string | null {
    const binaryName = process.platform === 'win32' ? 'gizzi-code.exe' : 'gizzi-code';
    const candidates = app.isPackaged
      ? [path.join(process.resourcesPath ?? '', 'bin', binaryName)]
      : [
          path.join(process.resourcesPath ?? '', 'bin', binaryName),
          path.join(app.getAppPath(), '..', '..', 'cmd', 'gizzi-code', 'dist', binaryName),
          path.join(__dirname, '..', '..', 'resources', 'bin', binaryName),
        ];
    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) return candidate;
    }
    log.error('[FabricWorker] gizzi-code binary not found. Searched:', candidates);
    return null;
  }

  /**
   * Start (or adopt) the worker. `token` is the fabric-transport principal
   * credential provisioned by the local ensure route; `apiUrl` points at
   * the loopback allternit-api the desktop just started.
   */
  async start(options: {
    token: string;
    apiUrl?: string;
    /** Existing model-router key (ALLTERNIT_OPERATOR_API_KEY) for agentic jobs. */
    operatorKey?: string | null;
    /** Granted folders (P2.3 confinement); empty array = default-deny. */
    trustedFolders?: string[];
  }): Promise<FabricWorkerState> {
    if (this.proc && this.proc.exitCode === null) {
      return this.status;
    }
    this.intentionalStop = false;
    const binaryPath = this.resolveBinaryPath();
    if (!binaryPath) {
      this.setStatus('down', 'gizzi-code bundle missing');
      return this.status;
    }
    const apiUrl = (options.apiUrl ?? URLS.API).replace(/\/+$/, '');
    const env: Record<string, string> = {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([, v]) => v !== undefined) as [string, string][],
      ),
      ALLTERNIT_GIZZI_TOKEN: options.token,
      ALLTERNIT_API_URL: apiUrl,
      GIZZI_COMPUTE_MODE: process.env.GIZZI_COMPUTE_MODE ?? 'local',
      // Agentic jobs (P2.2) reach the model through the existing router
      // (the api's own operator key); file tools confine to these grants.
      ...(options.operatorKey ? { ALLTERNIT_OPERATOR_API_KEY: options.operatorKey } : {}),
      ALLTERNIT_WORKER_TRUSTED_FOLDERS: JSON.stringify(options.trustedFolders ?? []),
    };
    // The token never reaches logs: we log everything except the env.
    log.info(`[FabricWorker] Spawning fabric worker (${binaryPath}) against ${apiUrl}`);
    this.spawnTimestamp = Date.now();
    this.setStatus('starting', 'Spawning worker…');

    const proc = spawn(binaryPath, ['fabric-worker'], {
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.proc = proc;
    let readinessSeen = false;

    proc.stdout?.on('data', (chunk: Buffer) => {
      const line = chunk.toString();
      if (!readinessSeen && line.includes(READINESS_LINE)) {
        readinessSeen = true;
        this.respawnAttempts = 0;
        this.setStatus('up', `Claiming on ${apiUrl}`);
      }
      for (const raw of line.split('\n')) {
        const trimmed = raw.trim();
        if (trimmed) log.info(`[FabricWorker:stdout] ${trimmed}`);
      }
    });
    proc.stderr?.on('data', (chunk: Buffer) => {
      log.warn(`[FabricWorker:stderr] ${chunk.toString().trim()}`);
    });
    proc.on('error', (error) => {
      log.error('[FabricWorker] Failed to spawn:', error);
      this.proc = null;
      this.scheduleRespawn(options, 'spawn error');
    });
    proc.on('exit', (code, signal) => {
      if (this.proc !== proc) return;
      this.proc = null;
      if (this.intentionalStop) {
        this.setStatus('stopped', signal ? `Stopped (${signal})` : 'Stopped');
        return;
      }
      const ranMs = Date.now() - this.spawnTimestamp;
      if (!readinessSeen || code !== 0) {
        log.warn(`[FabricWorker] Worker exited unexpectedly (code=${code}, signal=${signal}, ran=${ranMs}ms)`);
        this.setStatus('down', `Exited (code ${code ?? signal})`);
        this.scheduleRespawn(options, 'crash');
      } else {
        // Clean exit without an intentional stop — e.g. lease sweeper
        // shutdown. Treat as down and let the backoff respawn it.
        this.setStatus('down', 'Exited cleanly without request');
        this.scheduleRespawn(options, 'clean unexpected exit');
      }
    });
    return this.status;
  }

  private scheduleRespawn(options: { token: string; apiUrl?: string }, reason: string): void {
    if (this.intentionalStop || this.respawnTimer) return;
    const stableRun = Date.now() - this.spawnTimestamp > STABLE_RUN_MS;
    if (stableRun) this.respawnAttempts = 0;
    const step = Math.min(this.respawnAttempts, BACKOFF_STEPS_MS.length - 1);
    const baseDelay = BACKOFF_STEPS_MS[step];
    const delay = Math.round(baseDelay * (0.8 + Math.random() * 0.4));
    this.respawnAttempts += 1;
    log.info(`[FabricWorker] Respawning worker in ${delay}ms after ${reason} (attempt ${this.respawnAttempts})`);
    this.respawnTimer = setTimeout(() => {
      this.respawnTimer = null;
      this.start(options).catch((error) => {
        log.error('[FabricWorker] Respawn failed:', error);
        this.scheduleRespawn(options, 'respawn failure');
      });
    }, delay);
    this.respawnTimer.unref?.();
  }

  /** Graceful stop: SIGTERM; the worker finishes its in-flight claim and exits 0. */
  async stop(): Promise<void> {
    this.intentionalStop = true;
    if (this.respawnTimer) {
      clearTimeout(this.respawnTimer);
      this.respawnTimer = null;
    }
    const proc = this.proc;
    this.proc = null;
    if (proc && proc.exitCode === null) {
      this.setStatus('stopped', 'Stopping…');
      proc.kill('SIGTERM');
      await new Promise<void>((resolve) => {
        const timer = setTimeout(() => {
          if (proc.exitCode === null) proc.kill('SIGKILL');
          resolve();
        }, 8_000);
        proc.on('exit', () => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
    this.setStatus('stopped', 'Stopped');
  }
}

export const fabricWorkerManager = new FabricWorkerManager();
