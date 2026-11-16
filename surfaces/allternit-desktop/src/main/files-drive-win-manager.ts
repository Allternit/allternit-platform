/**
 * Files drive on Windows (plan step 24): runs one `allternit-files-win serve`
 * per Allternit computer, so each computer shows in File Explorer as
 * "Allternit · <computer>" (native/files-win).
 *
 * - Windows only, and only while the account's `files_drive` flag is on
 *   (cloud-api `GET /api/v1/me/flags` → `flags.files_drive`).
 * - Computers come from `GET /api/v1/runtime-devices`. Their ids are runtime
 *   device ids, which is what `computer_id` means in the Files contract
 *   (contracts/files-drive/SPEC.md, "Computer ids"). This PC itself, revoked
 *   devices and runtimes that are not computers (bot boxes, CLIs) are skipped.
 * - The token never goes on a command line or in a file: the helper starts with
 *   `--token-stdin` and gets `{"type":"token"}` lines on stdin, again whenever
 *   the session rotates.
 * - A crashed helper restarts with backoff (same pattern as
 *   subs-gateway-manager.ts). Quit stops every helper; turning the flag off
 *   stops them; signing out stops them and runs `unregister-all`, so another
 *   account never sees this one's locations.
 */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { app, Notification, shell } from 'electron';
import log from 'electron-log';
import { authManager } from './auth-manager.js';
import { URLS } from './config.js';
import { shortHostname } from './cowork-device.js';

const RECONCILE_MS = 5 * 60_000;
const SESSION_CHECK_MS = 30_000;
const BACKOFF_DELAYS_MS = [2_000, 5_000, 15_000, 60_000, 300_000];
const MAX_CRASH_RESTARTS = 8;
/** Runtime types that run allternit-api and so serve files/2. */
const FILES_RUNTIME_TYPES = new Set(['desktop', 'provisioned', 'vps', 'cloud']);

interface RuntimeDevice {
  id: string;
  name: string;
  runtimeType: string;
  status: string;
}

interface Candidate {
  kind: 'loopback' | 'mesh' | 'relay' | 'gateway';
  base_url: string;
  priority: number;
}

interface HelperConfig {
  computer_id: string;
  computer_name: string;
  device_id: string;
  device_name: string;
  transport: { kind: 'mesh' | 'relay'; base_url: string };
  cloud_api_base: string;
  icons_dir: string;
  state_dir: string;
}

interface Running {
  proc: ChildProcess | null;
  configPath: string;
  crashRestarts: number;
  stopping: boolean;
  timer: NodeJS.Timeout | null;
}

function cloudApiBaseUrl(): string {
  return (process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API).replace(/\/$/, '');
}

export function helperDir(): string {
  return app.isPackaged
    ? path.join(process.resourcesPath ?? '', 'files-win')
    : path.join(app.getAppPath(), 'resources', 'files-win');
}

export function stateDir(): string {
  const base = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
  return path.join(base, 'Allternit', 'files-win');
}

/** Which runtime devices get an Explorer location (exported for tests). */
export function filesComputers(devices: RuntimeDevice[], thisRuntimeId: string): RuntimeDevice[] {
  return devices.filter(
    (d) =>
      d.id &&
      d.id !== thisRuntimeId &&
      !d.id.startsWith('pc_') &&
      FILES_RUNTIME_TYPES.has(d.runtimeType) &&
      !['revoked', 'deleted', 'disabled'].includes(String(d.status).toLowerCase()),
  );
}

/** Mesh when this PC can reach it, else the relay (always available, never builds a pc_ URL). */
export function pickTransport(computerId: string, candidates: Candidate[], meshReachable: boolean): HelperConfig['transport'] {
  const sorted = [...candidates].sort((a, b) => a.priority - b.priority);
  const mesh = sorted.find((c) => c.kind === 'mesh');
  if (mesh && meshReachable) return { kind: 'mesh', base_url: mesh.base_url };
  const relay = sorted.find((c) => c.kind === 'relay');
  return {
    kind: 'relay',
    base_url: relay?.base_url ?? `${cloudApiBaseUrl()}/api/v1/runtime-devices/${encodeURIComponent(computerId)}/proxy`,
  };
}

async function cloudGet<T>(token: string, p: string): Promise<T> {
  const res = await fetch(`${cloudApiBaseUrl()}${p}`, {
    headers: { Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(15_000),
  });
  if (!res.ok) throw new Error(`${p}: ${res.status}`);
  return (await res.json()) as T;
}

async function meshAnswers(baseUrl: string): Promise<boolean> {
  try {
    const res = await fetch(`${baseUrl.replace(/\/$/, '')}/ping`, { signal: AbortSignal.timeout(2_000) });
    return res.ok;
  } catch {
    return false;
  }
}

export class FilesDriveWinManager {
  private running = new Map<string, Running>();
  private reconcileTimer: NodeJS.Timeout | null = null;
  private sessionTimer: NodeJS.Timeout | null = null;
  private lastToken: string | null = null;
  private signedInUser: string | null = null;
  private stopped = false;

  get exe(): string {
    return path.join(helperDir(), 'allternit-files-win.exe');
  }

  /** No-op off Windows or without the bundled helper. Never throws. */
  start(): void {
    if (process.platform !== 'win32') return;
    if (!fs.existsSync(this.exe)) {
      log.warn(`[FilesDrive] helper not bundled at ${this.exe}; Explorer locations are off`);
      return;
    }
    this.stopped = false;
    void this.reconcile();
    this.reconcileTimer = setInterval(() => void this.reconcile(), RECONCILE_MS);
    this.sessionTimer = setInterval(() => void this.checkSession(), SESSION_CHECK_MS);
  }

  /** Quit: stop every helper (locations stay registered for the next launch). */
  stop(): void {
    this.stopped = true;
    if (this.reconcileTimer) clearInterval(this.reconcileTimer);
    if (this.sessionTimer) clearInterval(this.sessionTimer);
    this.reconcileTimer = this.sessionTimer = null;
    for (const id of [...this.running.keys()]) this.stopOne(id);
  }

  private async checkSession(): Promise<void> {
    const session = await authManager.getSession().catch(() => null);
    if (!session) {
      if (this.signedInUser) await this.onSignedOut();
      return;
    }
    if (this.signedInUser && this.signedInUser !== session.userId) {
      // Another account signed in: the old account's locations go first.
      await this.onSignedOut();
    }
    if (!this.signedInUser) {
      this.signedInUser = session.userId;
      void this.reconcile();
    }
    if (session.accessToken !== this.lastToken) {
      this.lastToken = session.accessToken;
      for (const r of this.running.values()) this.sendToken(r);
    }
  }

  private async onSignedOut(): Promise<void> {
    log.info('[FilesDrive] signed out: stopping helpers and removing Explorer locations');
    this.signedInUser = null;
    this.lastToken = null;
    const procs = [...this.running.values()].map((r) => r.proc).filter((p): p is ChildProcess => !!p);
    for (const id of [...this.running.keys()]) this.stopOne(id);
    // Let each helper disconnect its sync root before the roots are removed.
    const until = Date.now() + 6_000;
    while (procs.some((p) => p.exitCode === null && p.signalCode === null) && Date.now() < until) {
      await new Promise((res) => setTimeout(res, 200));
    }
    const r = spawnSync(this.exe, ['unregister-all', '--state-dir', stateDir()], { windowsHide: true, timeout: 30_000 });
    if (r.status !== 0) log.warn(`[FilesDrive] unregister-all exited ${r.status}: ${String(r.stdout ?? '').slice(-400)}`);
  }

  private async reconcile(): Promise<void> {
    if (this.stopped) return;
    try {
      const session = await authManager.getSession();
      if (!session?.accessToken) return;
      if (!this.signedInUser) this.signedInUser = session.userId;
      this.lastToken = session.accessToken;
      const flags = await cloudGet<{ flags?: Record<string, unknown> }>(session.accessToken, '/api/v1/me/flags');
      if (flags.flags?.files_drive !== true) {
        if (this.running.size) log.info('[FilesDrive] files_drive is off for this account: stopping helpers');
        for (const id of [...this.running.keys()]) this.stopOne(id);
        return;
      }
      const list = await cloudGet<{ runtimes?: RuntimeDevice[] }>(session.accessToken, '/api/v1/runtime-devices');
      const wanted = filesComputers(list.runtimes ?? [], session.runtimeId);
      const wantedIds = new Set(wanted.map((d) => d.id));
      for (const id of [...this.running.keys()]) {
        if (!wantedIds.has(id)) {
          const r = this.running.get(id);
          this.stopOne(id);
          if (r) spawnSync(this.exe, ['unregister', '--config', r.configPath], { windowsHide: true, timeout: 30_000 });
        }
      }
      for (const d of wanted) {
        if (this.running.has(d.id)) continue;
        const config = await this.configFor(d, session.accessToken, session.runtimeId);
        this.startOne(d.id, config);
      }
      this.firstRun(wanted.length);
    } catch (err) {
      log.warn('[FilesDrive] reconcile failed; trying again later:', err);
    }
  }

  private async configFor(d: RuntimeDevice, token: string, thisRuntimeId: string): Promise<HelperConfig> {
    let candidates: Candidate[] = [];
    try {
      const ep = await cloudGet<{ candidates?: Candidate[] }>(token, `/api/v1/computer-files/${encodeURIComponent(d.id)}/endpoints`);
      candidates = ep.candidates ?? [];
    } catch (err) {
      log.warn(`[FilesDrive] endpoints for ${d.id} failed, using the relay:`, err);
    }
    const mesh = candidates.find((c) => c.kind === 'mesh');
    const transport = pickTransport(d.id, candidates, mesh ? await meshAnswers(mesh.base_url) : false);
    return {
      computer_id: d.id,
      computer_name: d.name || d.id,
      device_id: thisRuntimeId,
      device_name: shortHostname(),
      transport,
      cloud_api_base: cloudApiBaseUrl(),
      icons_dir: path.join(helperDir(), 'icons'),
      state_dir: stateDir(),
    };
  }

  private startOne(id: string, config: HelperConfig): void {
    const dir = path.join(stateDir(), 'config');
    fs.mkdirSync(dir, { recursive: true });
    // No secret in it: the token goes over stdin.
    const configPath = path.join(dir, `${id.replace(/[^A-Za-z0-9_.-]/g, '_')}.json`);
    fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
    const r: Running = this.running.get(id) ?? { proc: null, configPath, crashRestarts: 0, stopping: false, timer: null };
    r.configPath = configPath;
    this.running.set(id, r);
    this.spawn(id, r);
  }

  private spawn(id: string, r: Running): void {
    const proc = spawn(this.exe, ['serve', '--config', r.configPath, '--token-stdin'], {
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
    });
    r.proc = proc;
    r.stopping = false;
    this.sendToken(r);
    let buf = '';
    proc.stdout?.on('data', (d: Buffer) => {
      buf += d.toString();
      let nl: number;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line) continue;
        if (line.includes('"type":"ready"')) r.crashRestarts = 0;
        log.info(`[FilesDrive ${id}]`, line.slice(0, 500));
      }
    });
    proc.stderr?.on('data', (d: Buffer) => log.warn(`[FilesDrive ${id}]`, d.toString().trim().slice(0, 500)));
    proc.on('error', (err) => log.error(`[FilesDrive ${id}] could not start:`, err));
    proc.on('exit', (code) => {
      if (r.proc === proc) r.proc = null;
      if (r.stopping || this.stopped || !this.running.has(id)) return;
      log.warn(`[FilesDrive ${id}] exited (code ${code})`);
      if (r.crashRestarts >= MAX_CRASH_RESTARTS) {
        log.error(`[FilesDrive ${id}] crashed ${r.crashRestarts} times; not restarting until the app restarts`);
        return;
      }
      const delay = BACKOFF_DELAYS_MS[Math.min(r.crashRestarts, BACKOFF_DELAYS_MS.length - 1)];
      r.crashRestarts += 1;
      r.timer = setTimeout(() => {
        r.timer = null;
        if (!this.stopped && this.running.has(id)) this.spawn(id, r);
      }, delay);
    });
  }

  private sendToken(r: Running): void {
    if (!r.proc?.stdin?.writable || !this.lastToken) return;
    r.proc.stdin.write(`${JSON.stringify({ type: 'token', token: this.lastToken })}\n`);
  }

  private stopOne(id: string): void {
    const r = this.running.get(id);
    this.running.delete(id);
    if (!r) return;
    if (r.timer) clearTimeout(r.timer);
    r.stopping = true;
    const proc = r.proc;
    r.proc = null;
    if (!proc) return;
    try {
      // The helper disconnects its sync root cleanly on {"type":"stop"} or EOF.
      proc.stdin?.write(`${JSON.stringify({ type: 'stop' })}\n`);
      proc.stdin?.end();
    } catch {
      /* already gone */
    }
    setTimeout(() => {
      if (proc.exitCode === null) proc.kill();
    }, 5_000).unref();
  }

  /** First run (design pick 3A, Windows row): say once where the computers are. */
  private firstRun(count: number): void {
    if (count === 0) return;
    const marker = path.join(stateDir(), 'first-run-shown');
    if (fs.existsSync(marker)) return;
    fs.mkdirSync(stateDir(), { recursive: true });
    fs.writeFileSync(marker, new Date().toISOString());
    if (!Notification.isSupported()) return;
    const folder = path.join(os.homedir(), 'Allternit Files');
    const n = new Notification({
      title: 'Your computers are in File Explorer',
      body: `Open "Allternit Files" to browse ${count === 1 ? 'your computer' : `your ${count} computers`}. Browsing is free; a sleeping computer is only woken when you say so.`,
    });
    n.on('click', () => void shell.openPath(folder));
    n.show();
  }
}

export const filesDriveWinManager = new FilesDriveWinManager();
