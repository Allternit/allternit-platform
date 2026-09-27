/**
 * Gizzi Code Manager
 *
 * Manages the gizzi-code terminal server process — the AI runtime that powers
 * all agent sessions, conversations, tool calls, and provider routing.
 *
 * The platform surface connects to gizzi-code at TERMINAL_SERVER_URL
 * (default: http://127.0.0.1:4096). This manager owns that port.
 *
 * Bundled binary location:
 *   - Packaged app: resources/bin/gizzi-code
 *   - Dev monorepo:  cmd/gizzi-code/dist/gizzi-code
 */

import { app } from 'electron';
import { spawn, spawnSync, ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';
import log from 'electron-log';
import { PORTS, URLS } from './config.js';
import { meshManager } from './mesh-manager.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

const GIZZI_PORT = PORTS.GIZZI;
const HEALTH_TIMEOUT_MS = 30_000;

export interface GizziStartConfig {
  /** Credential persisted by the installed always-on daemon, when present. */
  existingPassword?: string | null;
  /** Allternit API bearer token gizzi-code should use for outbound platform
   * API calls (e.g. brain provisioning). In the desktop shell this is the
   * paired runtime-device token; the renderer never sees it. */
  apiToken?: string | null;
  /** Paired runtime-device id, used as the fabric relay endpoint. */
  runtimeId?: string | null;
  /** Extra env vars merged into the spawned process, e.g. connector sidecar
   * tokens from authManager.getConnectorSidecarEnvironment() — gizzi-code
   * hosts the Lens vault MCP server, which needs these to reach the
   * connector sidecar directly (see vault/connectors/sidecar.ts). */
  extraEnv?: Record<string, string>;
}

type GizziProbeResult = 'ready' | 'unauthorized' | 'unhealthy' | 'refused' | 'unreachable';

export class GizziManager {
  private static instance: GizziManager;
  private proc: ChildProcess | null = null;
  private password: string | null = null;
  private lastConfig: GizziStartConfig | null = null;
  private usingExternalRuntime = false;
  private stopping = false;
  private resolvedBinaryPath: string | null | undefined;
  /** Ring buffer of recent stderr lines for crash diagnostics (startup fail-fast). */
  private stderrLines: string[] = [];

  static getInstance(): GizziManager {
    if (!GizziManager.instance) {
      GizziManager.instance = new GizziManager();
    }
    return GizziManager.instance;
  }

  /** Start gizzi-code terminal server. Returns its base URL. */
  async start(config: GizziStartConfig = {}): Promise<string> {
    this.lastConfig = config;
    if (this.proc || this.usingExternalRuntime) {
      return this.getUrl();
    }

    const existingPassword = config.existingPassword ?? null;
    const existingRuntime = await this.probe(existingPassword);
    if (existingRuntime === 'ready') {
      this.password = existingPassword;
      this.usingExternalRuntime = true;
      log.info(`[GizziManager] Reusing existing Gizzi runtime at ${this.getUrl()}`);
      return this.getUrl();
    }
    if (existingRuntime === 'unauthorized') {
      throw new Error(
        `A password-protected Gizzi runtime is already using port ${GIZZI_PORT}, ` +
        'but its configured daemon credential did not match.'
      );
    }
    if (existingRuntime === 'unhealthy') {
      throw new Error(`Another unhealthy Gizzi runtime is already using port ${GIZZI_PORT}`);
    }
    if (existingRuntime === 'unreachable') {
      // Timeout/reset rather than ECONNREFUSED — possibly a runtime mid-start
      // (e.g. the always-on daemon launchd just spawned). Poll patiently for
      // the health window to adopt it; a dead port short-circuits above via
      // 'refused', so the cold-boot path never pays this wait.
      const adopted = await this.waitForExistingRuntime(existingPassword, HEALTH_TIMEOUT_MS);
      if (adopted === 'ready') {
        this.password = existingPassword;
        this.usingExternalRuntime = true;
        log.info(`[GizziManager] Reusing existing Gizzi runtime at ${this.getUrl()}`);
        return this.getUrl();
      }
      if (adopted === 'unauthorized') {
        throw new Error(
          `A password-protected Gizzi runtime is already using port ${GIZZI_PORT}, ` +
          'but its configured daemon credential did not match.'
        );
      }
      if (adopted === 'unhealthy') {
        throw new Error(`Another unhealthy Gizzi runtime is already using port ${GIZZI_PORT}`);
      }
    }

    const binaryPath = this.resolveBinaryPath();
    if (!binaryPath) {
      throw new Error(
        'gizzi-code binary not found. Run the desktop build pipeline:\n' +
        '  scripts/build-desktop.sh'
      );
    }

    // The managed runtime is loopback-only and reachable only through the
    // signed desktop/API brokers. There is no second user-facing Gizzi login or
    // persistent Basic credential. `password` remains only for adopting a
    // legacy daemon until it is reinstalled.
    this.password = null;

    const env: Record<string, string> = {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([, v]) => v !== undefined) as [string, string][]
      ),
      // Gizzi terminal server config
      GIZZI_PORT: String(GIZZI_PORT),
      GIZZI_HOST: '127.0.0.1',
      // Point at allternit-api for operator-level routes (vm-session, rails, etc.)
      ALLTERNIT_API_URL: URLS.API,
      NODE_ENV: 'production',
      ...(config.apiToken ? { ALLTERNIT_API_TOKEN: config.apiToken } : {}),
      ...(config.runtimeId ? { GIZZI_RUNTIME_ID: config.runtimeId } : {}),
      GIZZI_PLATFORM_API_URL: URLS.CLOUD_API,
      ...(config.extraEnv ?? {}),
    };

    const meshBin = meshManager.resolveSidecarPath();
    if (meshBin) {
      env.GIZZI_MESH_NODE_BIN = meshBin;
    }
    try {
      const enrollment = await meshManager.enrollForGizzi();
      if (enrollment) {
        env.GIZZI_MESH_AUTH_KEY = enrollment.authKey;
        env.GIZZI_MESH_CONTROL_URL = enrollment.controlUrl;
        log.info('[GizziManager] Fabric mesh enrollment ready');
      }
    } catch (error) {
      log.warn('[GizziManager] Fabric mesh enrollment skipped:', error);
    }

    log.info(`[GizziManager] Starting gizzi-code on port ${GIZZI_PORT} from ${binaryPath}`);

    // Point gizzi at the vendored allternit-mux (auto-spawned for /pty);
    // when absent, gizzi falls back to its own resolution (vendor tree, PATH,
    // repo target builds).
    const muxBinaryPath = this.resolveMuxBinaryPath(binaryPath);
    if (muxBinaryPath) {
      env.ALLTERNIT_MUX_BIN = muxBinaryPath;
      log.info(`[GizziManager] allternit-mux at: ${muxBinaryPath}`);
    }

    const serveArgs = ['serve', '--port', String(GIZZI_PORT), '--hostname', '127.0.0.1', '--print-logs'];
    if (env.GIZZI_MESH_AUTH_KEY) {
      serveArgs.push('--mesh');
    }
    const proc = spawn(binaryPath, serveArgs, {
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
      // Own process group so stop() can reap gizzi and its session children
      // without signalling Electron.
      detached: process.platform !== 'win32',
    });
    this.proc = proc;

    proc.stdout?.on('data', (d: Buffer) =>
      log.info('[Gizzi]', d.toString().trim())
    );
    proc.stderr?.on('data', (d: Buffer) => {
      log.warn('[Gizzi]', d.toString().trim());
      this.pushStderrLine(d.toString());
    });
    proc.on('exit', (code) => {
      log.warn(`[GizziManager] exited (code ${code})`);
      const intentionalStop = this.stopping;
      this.stopping = false;
      if (this.proc === proc) {
        this.proc = null;
        this.password = null;
      }

      // Auto-restart logic
      if (!intentionalStop && this.lastConfig && (app.isPackaged || process.env.NODE_ENV === 'production')) {
        log.info('[GizziManager] AI runtime crashed unexpectedly, respawning in 1s...');
        setTimeout(() => {
          if (this.lastConfig) {
            this.start(this.lastConfig).catch(e => 
              log.error('[GizziManager] Failed to auto-restart AI runtime:', e)
            );
          }
        }, 1000);
      }
    });

    await this.waitUntilReady();

    log.info(`[GizziManager] Ready at ${this.getUrl()}`);
    return this.getUrl();
  }

  /**
   * Stop the managed gizzi-code process tree.
   * `reapExternal` also kills whatever is still listening on the Gizzi port
   * (adopted daemon or a leftover from a previous session).
   */
  stop(opts: { reapExternal?: boolean } = {}): void {
    this.lastConfig = null;
    const proc = this.proc;
    const pid = proc?.pid;
    if (this.usingExternalRuntime && !opts.reapExternal) {
      log.info('[GizziManager] Detaching from always-on Gizzi runtime');
      this.usingExternalRuntime = false;
      this.password = null;
      return;
    }
    if (proc || opts.reapExternal) {
      log.info('[GizziManager] Stopping…');
      this.stopping = true;
      if (pid) killProcessTree(pid);
      else if (proc) {
        try { proc.kill('SIGTERM'); } catch { /* already gone */ }
      }
      if (opts.reapExternal) {
        killListenerOnPort(GIZZI_PORT);
      }
      this.proc = null;
      this.password = null;
      this.usingExternalRuntime = false;
    }
  }

  getUrl(): string {
    return `http://127.0.0.1:${GIZZI_PORT}`;
  }

  /** Basic-auth header for the platform server to pass to gizzi-code */
  getAuthHeader(): string | undefined {
    if (!this.password) return undefined;
    const token = Buffer.from(`gizzi:${this.password}`).toString('base64');
    return `Basic ${token}`;
  }

  /** Password used for GIZZI_PASSWORD env on the Next.js standalone server */
  getPassword(): string | null {
    return this.password;
  }

  /** True when a spawned or adopted Gizzi runtime is available. */
  isRunning(): boolean {
    return this.proc !== null || this.usingExternalRuntime;
  }

  // ─── Private ──────────────────────────────────────────────────────────────

  private async waitUntilReady(): Promise<void> {
    const deadline = Date.now() + HEALTH_TIMEOUT_MS;
    const url = `http://127.0.0.1:${GIZZI_PORT}/v1/global/health`;
    const authHeader = this.getAuthHeader();
    const child = this.proc;
    const crashState = { crashed: false, code: null as number | null };
    const onExit = (code: number | null) => {
      crashState.crashed = true;
      crashState.code = code;
    };
    child?.once('exit', onExit);

    try {
      while (Date.now() < deadline) {
        if (crashState.crashed) {
          throw new Error(
            `gizzi-code exited during startup (code ${crashState.code}): ${this.recentStderr().join(' | ')}`,
          );
        }
        try {
          const res = await fetch(url, {
            signal: AbortSignal.timeout(2000),
            headers: authHeader ? { Authorization: authHeader } : undefined,
          });
          if (res.ok || res.status === 404) {
            // 404 = server up, no health route (fine)
            return;
          }
        } catch {
          // Not ready yet
        }
        await new Promise(r => setTimeout(r, 200));
      }
      throw new Error(`gizzi-code did not start within ${HEALTH_TIMEOUT_MS / 1000}s`);
    } finally {
      if (child) child.removeListener('exit', onExit);
    }
  }

  /** Patiently poll for a runtime that may be mid-start, to adopt it. */
  private async waitForExistingRuntime(password: string | null, timeoutMs: number): Promise<GizziProbeResult> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const result = await this.probe(password);
      if (result === 'ready' || result === 'unauthorized' || result === 'unhealthy') {
        return result;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
    return 'unreachable';
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

  private async probe(password: string | null): Promise<GizziProbeResult> {
    const authHeader = password
      ? `Basic ${Buffer.from(`gizzi:${password}`).toString('base64')}`
      : undefined;
    try {
      const res = await fetch(`${this.getUrl()}/v1/global/health`, {
        signal: AbortSignal.timeout(1000),
        headers: authHeader ? { Authorization: authHeader } : undefined,
      });
      if (res.ok || res.status === 404) return 'ready';
      if (res.status === 401) return 'unauthorized';
      return 'unhealthy';
    } catch (probeErr) {
      // ECONNREFUSED means nothing is (or will be) listening — the caller
      // spawns immediately instead of paying a patient poll for a port that
      // can never answer.
      if ((probeErr as { cause?: { code?: string } })?.cause?.code === 'ECONNREFUSED') {
        return 'refused';
      }
      return 'unreachable';
    }
  }

  private resolveBinaryPath(): string | null {
    if (this.resolvedBinaryPath !== undefined) {
      return this.resolvedBinaryPath;
    }

    const binaryName = process.platform === 'win32' ? 'gizzi-code.exe' : 'gizzi-code';

    const candidates = app.isPackaged
      ? [
          path.join(process.resourcesPath ?? '', 'bin', binaryName),
        ]
      : [
          path.join(process.resourcesPath ?? '', 'bin', binaryName),
          path.join(app.getAppPath(), '..', '..', 'cmd', 'gizzi-code', 'dist', binaryName),
          path.join(__dirname, '..', '..', 'resources', 'bin', binaryName),
        ];

    for (const p of candidates) {
      if (fs.existsSync(p)) {
        this.resolvedBinaryPath = p;
        log.info(`[GizziManager] Found binary at: ${p}`);
        return p;
      }
    }

    log.error('[GizziManager] gizzi-code binary not found. Searched:', candidates);
    this.resolvedBinaryPath = null;
    return null;
  }

  /** Locate the vendored allternit-mux sibling of the resolved gizzi binary. */
  private resolveMuxBinaryPath(gizziBinaryPath: string): string | null {
    const muxName = process.platform === 'win32' ? 'allternit-mux.exe' : 'allternit-mux';
    const gizziDir = path.dirname(gizziBinaryPath);
    const platformArch = `${process.platform}-${process.arch}`;
    const candidates = [
      path.join(gizziDir, muxName), // resources/bin or dist sibling
      path.join(gizziDir, 'vendor', 'allternit-mux', platformArch, muxName), // vendor tree
      app.isPackaged ? path.join(process.resourcesPath ?? '', 'bin', muxName) : '',
    ].filter(Boolean);
    for (const p of candidates) {
      if (fs.existsSync(p)) return p;
    }
    return null;
  }
}

export const gizziManager = GizziManager.getInstance();

function descendantPids(pid: number): number[] {
  const result = spawnSync('pgrep', ['-P', String(pid)], { encoding: 'utf8' });
  if (result.status !== 0) return [];
  const children = (result.stdout || '')
    .split(/\s+/)
    .map((s) => Number(s.trim()))
    .filter((n) => Number.isFinite(n) && n > 0);
  return children.flatMap((child) => [...descendantPids(child), child]);
}

function killProcessTree(pid: number): void {
  if (!pid) return;
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/pid', String(pid), '/T', '/F'], {
      stdio: 'ignore',
      windowsHide: true,
    });
    return;
  }
  try {
    process.kill(-pid, 'SIGTERM');
  } catch {
    const descendants = descendantPids(pid);
    for (const child of descendants.reverse()) {
      try { process.kill(child, 'SIGTERM'); } catch { /* gone */ }
    }
    try { process.kill(pid, 'SIGTERM'); } catch { /* gone */ }
  }
  setTimeout(() => {
    try { process.kill(-pid, 'SIGKILL'); } catch { /* gone */ }
    try { process.kill(pid, 'SIGKILL'); } catch { /* gone */ }
  }, 2500);
}

function killListenerOnPort(port: number): void {
  if (process.platform === 'win32') {
    const out = spawnSync(
      'powershell.exe',
      [
        '-NoProfile',
        '-Command',
        `(Get-NetTCPConnection -LocalPort ${port} -State Listen -ErrorAction SilentlyContinue).OwningProcess`,
      ],
      { encoding: 'utf8', windowsHide: true, timeout: 4000 },
    );
    const pids = new Set(
      (out.stdout || '')
        .split(/\s+/)
        .map((s) => Number(s.trim()))
        .filter((n) => Number.isFinite(n) && n > 0),
    );
    for (const pid of pids) killProcessTree(pid);
    return;
  }
  const out = spawnSync('lsof', ['-ti', `tcp:${port}`, '-sTCP:LISTEN'], {
    encoding: 'utf8',
    timeout: 4000,
  });
  const pids = new Set(
    (out.stdout || '')
      .split(/\s+/)
      .map((s) => Number(s.trim()))
      .filter((n) => Number.isFinite(n) && n > 0),
  );
  for (const pid of pids) killProcessTree(pid);
}
