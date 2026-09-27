/**
 * Phone Remote Manager
 *
 * Owns the lifecycle of the phone-remote server (surfaces/phone-remote,
 * plain Node, zero deps) that serves ScreenCaptureKit JPEG frames on
 * 127.0.0.1:8477. The Fabric desktop viewer proxies /frame + /hello there
 * (see auth-manager.ts); before this manager existed nothing spawned or
 * supervised that server — the 2026-09-13 incident was a manually-started
 * instance serving a stale frame for hours with no error anywhere.
 *
 * Lifecycle:
 *   - Spawn: if /healthz on :8477 is dark, spawn the bundled server with
 *     Electron's own Node (ELECTRON_RUN_AS_NODE=1, same trick as the
 *     connector sidecar) and PORT=8477. stdio goes to phone-remote.log in
 *     the same directory as main.log. Non-fatal: the app must still render
 *     when the server cannot start.
 *   - Adopt, don't kill: if :8477 already answers /healthz we spawn nothing
 *     and stop nothing — the foreign instance (e.g. a manual dev start) is
 *     logged with its owning PID and adopted. This warning is the
 *     diagnostic that was missing during the incident.
 *   - Supervise: if OUR child exits while the app is running, respawn with
 *     backoff (1s → 2s → 5s → 30s cap, keep trying). The server exits
 *     non-zero when its capture watchdog gives up, so a frozen capture
 *     becomes a full server restart here.
 *   - Quit: SIGTERM our child on teardown, escalate to SIGKILL after 3s.
 *     Foreign instances are never touched.
 */

import { app } from 'electron';
import { spawn, execFileSync, ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import log from 'electron-log';

const __dirname = dirname(fileURLToPath(import.meta.url));

const PORT = 8477;
const BASE_URL = `http://127.0.0.1:${PORT}`;
const PROBE_TIMEOUT_MS = 800;
const HEALTH_TIMEOUT_MS = 8_000;
const KILL_GRACE_MS = 3_000;
// 1s → 2s → 5s → 30s, then keep trying at 30s.
const BACKOFF_STEPS_MS = [1000, 2000, 5000, 30000];

export class PhoneRemoteManager {
  private static instance: PhoneRemoteManager;
  private proc: ChildProcess | null = null;
  private stopping = false;
  private respawnAttempts = 0;
  private respawnTimer: ReturnType<typeof setTimeout> | null = null;
  private killTimer: ReturnType<typeof setTimeout> | null = null;
  /** True when we adopted a foreign :8477 listener — we must not stop it. */
  private adoptedForeign = false;
  private resolvedScriptPath: string | null | undefined;

  static getInstance(): PhoneRemoteManager {
    if (!PhoneRemoteManager.instance) {
      PhoneRemoteManager.instance = new PhoneRemoteManager();
    }
    return PhoneRemoteManager.instance;
  }

  /**
   * Ensure a phone-remote server is serving :8477. Spawns and supervises our
   * own child when the port is dark; adopts (and warns about) a foreign one.
   * Never throws — failures are logged and left for the next launch/viewer
   * to surface, matching the other optional sidecars.
   */
  async ensureStarted(): Promise<void> {
    if (this.proc || this.adoptedForeign) return;

    if (await this.probeHealthz()) {
      this.adoptForeign();
      return;
    }

    const scriptPath = this.resolveScriptPath();
    if (!scriptPath) {
      log.warn(
        '[PhoneRemote] phone-remote server not found — the Fabric phone viewer will show nothing. ' +
          'Packaged builds should never see this; dev trees need surfaces/phone-remote/server/index.mjs.',
      );
      return;
    }

    // Port ownership guard: something may have bound :8477 between the first
    // probe and now. Race-safe enough for this purpose (a spawn against a
    // taken port would fail its health wait and supervise-restart anyway).
    if (await this.probeHealthz()) {
      this.adoptForeign();
      return;
    }

    this.spawn(scriptPath);

    try {
      await this.waitForHealthz();
      this.respawnAttempts = 0;
      log.info(`[PhoneRemote] phone-remote server ready at ${BASE_URL} (log: ${this.logFilePath()})`);
    } catch (error) {
      log.warn('[PhoneRemote] phone-remote server did not become healthy; viewer stays dark until it does:', error);
      // The exit handler supervises the child; a spawn that died instantly is
      // already being restarted with backoff.
    }
  }

  /**
   * Teardown: SIGTERM our child, escalate to SIGKILL after KILL_GRACE_MS.
   * Foreign instances are never touched (adopt-don't-kill).
   */
  stop(): void {
    this.stopping = true;
    if (this.respawnTimer) {
      clearTimeout(this.respawnTimer);
      this.respawnTimer = null;
    }
    if (this.killTimer) {
      clearTimeout(this.killTimer);
      this.killTimer = null;
    }
    if (this.proc) {
      const child = this.proc;
      this.proc = null;
      log.info('[PhoneRemote] Stopping phone-remote server…');
      try {
        child.kill('SIGTERM');
      } catch (error) {
        log.warn('[PhoneRemote] SIGTERM failed:', error);
      }
      this.killTimer = setTimeout(() => {
        this.killTimer = null;
        // `killed` is true from the moment kill() is called, so test actual
        // exit state: still running after the grace period → escalate.
        if (child.exitCode === null && child.signalCode === null) {
          log.warn('[PhoneRemote] phone-remote server ignored SIGTERM — SIGKILL');
          try {
            child.kill('SIGKILL');
          } catch (error) {
            log.warn('[PhoneRemote] SIGKILL failed:', error);
          }
        }
      }, KILL_GRACE_MS);
      this.killTimer.unref?.();
    }
  }

  getUrl(): string {
    return BASE_URL;
  }

  /** True when a server we do NOT own is serving :8477. */
  isForeign(): boolean {
    return this.adoptedForeign;
  }

  private adoptForeign(): void {
    this.adoptedForeign = true;
    const pid = this.probePortOwnerPid();
    log.warn(
      `[PhoneRemote] port ${PORT} already serves /healthz (owner pid: ${pid ?? 'unknown'}) — adopting it. ` +
        'The desktop will NOT supervise or stop this instance; a stale or frozen server on this port is ' +
        'the owner process’s problem. Kill it and relaunch the app to let the desktop own the lifecycle.',
    );
  }

  private spawn(scriptPath: string): void {
    this.stopping = false;
    const logFile = this.logFilePath();
    const out = fs.openSync(logFile, 'a');
    log.info(`[PhoneRemote] Starting phone-remote server: ${process.execPath} ${scriptPath} (log: ${logFile})`);

    const proc = spawn(process.execPath, [scriptPath], {
      env: {
        ...Object.fromEntries(
          Object.entries(process.env).filter(([, v]) => v !== undefined) as [string, string][]
        ),
        ELECTRON_RUN_AS_NODE: '1',
        PORT: String(PORT),
        NODE_ENV: 'production',
      },
      stdio: ['ignore', out, out],
      windowsHide: true,
    });
    fs.closeSync(out);
    this.proc = proc;

    proc.on('exit', (code) => {
      if (this.proc === proc) this.proc = null;
      if (this.stopping) return;
      const delay = BACKOFF_STEPS_MS[Math.min(this.respawnAttempts, BACKOFF_STEPS_MS.length - 1)];
      this.respawnAttempts += 1;
      log.info(
        `[PhoneRemote] phone-remote server exited (code ${code}), respawning in ${delay / 1000}s ` +
          `(attempt ${this.respawnAttempts})…`,
      );
      this.respawnTimer = setTimeout(() => {
        this.respawnTimer = null;
        void this.ensureStarted().catch((error) =>
          log.error('[PhoneRemote] failed to restart phone-remote server:', error),
        );
      }, delay);
      this.respawnTimer.unref?.();
    });
  }

  private async probeHealthz(): Promise<boolean> {
    try {
      const res = await fetch(`${BASE_URL}/healthz`, { signal: AbortSignal.timeout(PROBE_TIMEOUT_MS) });
      return res.ok;
    } catch {
      return false;
    }
  }

  private async waitForHealthz(): Promise<void> {
    const deadline = Date.now() + HEALTH_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (!this.proc) {
        throw new Error('phone-remote server exited before becoming healthy (see phone-remote.log)');
      }
      if (await this.probeHealthz()) return;
      await new Promise((r) => setTimeout(r, 250));
    }
    throw new Error(`phone-remote server did not answer /healthz within ${HEALTH_TIMEOUT_MS / 1000}s`);
  }

  /** Best-effort PID of whatever listens on :8477 (lsof on macOS/Linux). */
  private probePortOwnerPid(): number | null {
    try {
      const out = execFileSync('lsof', ['-nP', `-iTCP:${PORT}`, '-sTCP:LISTEN', '-t'], { encoding: 'utf8' });
      const pid = Number(out.trim().split('\n')[0]);
      return Number.isFinite(pid) && pid > 0 ? pid : null;
    } catch {
      return null; // lsof missing (Windows) or nothing listening
    }
  }

  private logFilePath(): string {
    // Same directory main.log lives in (see unified-main.ts log.transports).
    return path.join(app.getPath('userData'), 'phone-remote.log');
  }

  private resolveScriptPath(): string | null {
    if (this.resolvedScriptPath !== undefined) {
      return this.resolvedScriptPath;
    }

    const packagedScript = path.join(process.resourcesPath ?? '', 'phone-remote', 'server', 'index.mjs');
    const candidates = app.isPackaged
      ? [packagedScript]
      : [
          // Dev monorepo: __dirname is dist/main; up four levels is the repo
          // root. A graceful skip+log happens when the worktree lacks the
          // surface (e.g. sparse checkouts).
          path.resolve(__dirname, '..', '..', '..', '..', 'surfaces', 'phone-remote', 'server', 'index.mjs'),
          packagedScript,
        ];

    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) {
        this.resolvedScriptPath = candidate;
        return candidate;
      }
    }

    this.resolvedScriptPath = null;
    return null;
  }
}

export const phoneRemoteManager = PhoneRemoteManager.getInstance();
