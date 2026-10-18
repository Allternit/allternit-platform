/**
 * Subscription gateway inside Allternit Desktop (Eoj, 2026-10-17: bundle it).
 *
 * Runs services/subscription-gateway, bundled by
 * scripts/prepare-subs-gateway.cjs into resources/subs-gateway, on Electron's
 * own Node (ELECTRON_RUN_AS_NODE=1; the gateway uses node:sqlite, no native
 * module). It serves the lanes that need this computer:
 *
 *   - phone lanes, through Desktop's phone gateway (~/.allternit/phone-gateway.json),
 *   - desktop-app lanes (the ChatGPT, Claude and Kimi apps; macOS ax-bridge),
 *   - browser lanes only when the person turns them on, in a hidden Chrome on
 *     its own profile (SUBS_GATEWAY_LOCAL_DESKTOP=1; the gateway decides).
 *
 * allternit-api gets ALLTERNIT_LOCAL_SUBS_GATEWAY=fallback: a bound, running
 * Sessions computer answers first and this gateway answers when there is none.
 *
 * Not started where the machine already runs its own gateway: an Allternit
 * cloud computer sets ALLTERNIT_LOCAL_SUBS_GATEWAY=1 for the systemd unit
 * (cmd/allternit-computer-cloud/guest/build-cloud-computer-image.sh), and the
 * API env passes that through unchanged.
 *
 * State lives where the gateway and `gizzi subs` already look:
 * ~/.allternit/subscriptions (SUBS_GATEWAY_STATE_DIR overrides). Secrets use
 * the gateway's 0600 file store there, the same as Sessions computers, because
 * allternit-api reads the gateway token from keychain.json.
 */

import { app } from 'electron';
import type { ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as net from 'net';
import * as os from 'os';
import * as path from 'path';
import log from 'electron-log';
import { URLS } from './config.js';
import { spawnSidecar } from './process-lifeline.js';

export const DEFAULT_GATEWAY_PORT = 7788;
const READY_TIMEOUT_MS = 60_000;
const MAX_CRASH_RESTARTS = 5;
const BACKOFF_DELAYS_MS = [1000, 2000, 4000, 8000, 16000];
const READY_RE = /listening on http:\/\/[^:]+:(\d+)/;

export type SubsGatewayStatus = 'stopped' | 'starting' | 'running' | 'external' | 'degraded';

/** The machine runs its own gateway (cloud computer image); Desktop must not start a second one. */
export function machineHasOwnGateway(env: NodeJS.ProcessEnv = process.env): boolean {
  return env.ALLTERNIT_LOCAL_SUBS_GATEWAY === '1' || env.ALLTERNIT_LOCAL_SUBS_GATEWAY === 'true';
}

export function gatewayStateDir(env: NodeJS.ProcessEnv = process.env): string {
  return env.SUBS_GATEWAY_STATE_DIR || path.join(os.homedir(), '.allternit', 'subscriptions');
}

export interface GatewayLayout {
  /** resources/subs-gateway */
  root: string;
  entry: string;
  /** macOS accessibility helper, when shipped. */
  axBridge: string | null;
}

/** Packaged: <resources>/subs-gateway. Dev: the staged bundle under the desktop package. */
export function resolveGatewayLayout(candidates: string[], exists: (p: string) => boolean = fs.existsSync): GatewayLayout | null {
  for (const root of candidates) {
    const entry = path.join(root, 'src', 'main.js');
    if (!exists(entry)) continue;
    const ax = path.join(root, 'bin', 'ax-bridge');
    return { root, entry, axBridge: process.platform === 'darwin' && exists(ax) ? ax : null };
  }
  return null;
}

export function buildGatewayEnv(opts: {
  base: NodeJS.ProcessEnv;
  layout: GatewayLayout;
  stateDir: string;
  port: number;
  apiBase: string;
  phoneGatewayFile: string;
}): Record<string, string> {
  const { base, layout, stateDir, port } = opts;
  const env: Record<string, string> = Object.fromEntries(
    Object.entries(base).filter(([, v]) => v !== undefined) as [string, string][],
  );
  return {
    ...env,
    ELECTRON_RUN_AS_NODE: '1',
    NODE_ENV: 'production',
    SUBS_GATEWAY_STATE_DIR: stateDir,
    SUBS_GATEWAY_KEYCHAIN: 'file',
    SUBS_GATEWAY_TCP: '1',
    SUBS_GATEWAY_TCP_HOST: '127.0.0.1',
    SUBS_GATEWAY_TCP_PORT: String(port),
    SUBS_GATEWAY_ADAPTERS_DIR: path.join(layout.root, 'adapters'),
    SUBS_GATEWAY_MIGRATIONS_DIR: path.join(layout.root, 'migrations'),
    SUBS_GATEWAY_BUNDLE_ROOT: layout.root,
    SUBS_GATEWAY_API_BASE: opts.apiBase,
    // The person's own computer: browser lanes are off until they turn them
    // on, and then run in a hidden Chrome (src/worker/hidden_chrome.ts).
    SUBS_GATEWAY_LOCAL_DESKTOP: '1',
    SUBS_GATEWAY_PHONE_GATEWAY_FILE: opts.phoneGatewayFile,
    ...(layout.axBridge && !env.SUBS_GATEWAY_AX_BRIDGE_BIN ? { SUBS_GATEWAY_AX_BRIDGE_BIN: layout.axBridge } : {}),
  };
}

/** What allternit-api needs to reach this gateway (subscription_routes.rs local_gateway). */
export function apiEnvironmentFor(port: number, stateDir: string): Record<string, string> {
  return {
    ALLTERNIT_LOCAL_SUBS_GATEWAY: 'fallback',
    ALLTERNIT_LOCAL_SUBS_GATEWAY_PORT: String(port),
    ALLTERNIT_LOCAL_SUBS_GATEWAY_STATE_DIR: stateDir,
  };
}

/** 7788 when free (what `gizzi subs` and the docs expect), else any free loopback port. */
export async function pickPort(preferred = DEFAULT_GATEWAY_PORT): Promise<number> {
  const tryListen = (port: number) =>
    new Promise<number | null>((resolve) => {
      const srv = net.createServer();
      srv.once('error', () => resolve(null));
      srv.listen(port, '127.0.0.1', () => {
        const got = (srv.address() as net.AddressInfo).port;
        srv.close(() => resolve(got));
      });
    });
  return (await tryListen(preferred)) ?? (await tryListen(0)) ?? preferred;
}

export class SubsGatewayManager {
  private proc: ChildProcess | null = null;
  private stopping = false;
  private status: SubsGatewayStatus = 'stopped';
  private port: number | null = null;
  private layout: GatewayLayout | null = null;
  private crashRestarts = 0;
  private everReady = false;

  getStatus(): SubsGatewayStatus {
    return this.status;
  }

  /** Env for allternit-api, once prepare() reserved a port. Empty when the machine has its own gateway or no bundle. */
  getApiEnvironment(): Record<string, string> {
    if (this.status === 'external' || this.port === null) return {};
    return apiEnvironmentFor(this.port, gatewayStateDir());
  }

  /**
   * Find the bundle and reserve the port, so allternit-api can be started
   * with the gateway's address before the gateway itself is up (the API reads
   * the token from keychain.json on each request). Fast; never throws.
   */
  async prepare(): Promise<void> {
    if (this.port !== null || this.status === 'external') return;
    if (machineHasOwnGateway()) {
      this.status = 'external';
      log.info('[SubsGateway] this machine runs its own subscription gateway; not starting the bundled one');
      return;
    }
    this.layout = resolveGatewayLayout(
      app.isPackaged
        ? [path.join(process.resourcesPath ?? '', 'subs-gateway')]
        : [path.join(app.getAppPath(), 'resources', 'subs-gateway')],
    );
    if (!this.layout) {
      log.warn('[SubsGateway] bundle not found (run npm run prepare:subs-gateway); phone and desktop-app lanes need a Sessions computer');
      this.status = 'degraded';
      return;
    }
    this.port = await pickPort();
  }

  /**
   * Start the gateway and wait until it listens. Never throws: a missing
   * bundle or a failed start leaves Desktop working without its own gateway
   * (a Sessions computer still serves), and the status says why.
   */
  async start(): Promise<SubsGatewayStatus> {
    await this.prepare();
    if (this.status === 'external' || !this.layout || this.port === null) return this.status;
    if (this.proc) return this.status;
    const stateDir = gatewayStateDir();
    fs.mkdirSync(stateDir, { recursive: true, mode: 0o700 });
    const env = buildGatewayEnv({
      base: process.env,
      layout: this.layout,
      stateDir,
      port: this.port,
      apiBase: URLS.API,
      phoneGatewayFile: path.join(os.homedir(), '.allternit', 'phone-gateway.json'),
    });
    this.status = 'starting';
    log.info(`[SubsGateway] starting on 127.0.0.1:${this.port} (state ${stateDir})`);
    const proc = spawnSidecar(process.execPath, [this.layout.entry], {
      env,
      cwd: this.layout.root,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    });
    this.proc = proc;
    let tail = '';
    const ready = new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => resolve(false), READY_TIMEOUT_MS);
      const onData = (d: Buffer) => {
        const text = d.toString();
        tail = (tail + text).slice(-4000);
        for (const line of text.split('\n')) if (line.trim()) log.info('[SubsGateway]', line.trim());
        if (READY_RE.test(tail)) {
          clearTimeout(timer);
          resolve(true);
        }
      };
      proc.stdout?.on('data', onData);
      proc.stderr?.on('data', onData);
      proc.once('exit', () => {
        clearTimeout(timer);
        resolve(false);
      });
    });
    proc.on('exit', (code) => {
      const intentional = this.stopping;
      this.stopping = false;
      if (this.proc === proc) this.proc = null;
      if (intentional) return;
      log.warn(`[SubsGateway] exited (code ${code})`);
      if (this.everReady) this.scheduleRestart();
    });
    if (!(await ready)) {
      log.error(`[SubsGateway] did not start: ${tail.trim().split('\n').slice(-5).join(' | ')}`);
      if (this.proc === proc) {
        this.stopping = true;
        proc.kill('SIGTERM');
        this.proc = null;
      }
      this.status = 'degraded';
      return this.status;
    }
    this.everReady = true;
    this.crashRestarts = 0;
    this.status = 'running';
    log.info(`[SubsGateway] running on 127.0.0.1:${this.port}`);
    return this.status;
  }

  stop(): void {
    if (this.proc) {
      this.stopping = true;
      this.proc.kill('SIGTERM');
      this.proc = null;
    }
    if (this.status !== 'external') this.status = 'stopped';
  }

  private scheduleRestart(): void {
    if (this.crashRestarts >= MAX_CRASH_RESTARTS) {
      this.status = 'degraded';
      log.error(`[SubsGateway] crashed ${this.crashRestarts} times; not restarting until the app restarts`);
      return;
    }
    const delay = BACKOFF_DELAYS_MS[Math.min(this.crashRestarts, BACKOFF_DELAYS_MS.length - 1)];
    this.crashRestarts += 1;
    this.status = 'starting';
    // Same port: allternit-api was started with it.
    setTimeout(() => void this.start(), delay);
  }
}

export const subsGatewayManager = new SubsGatewayManager();
