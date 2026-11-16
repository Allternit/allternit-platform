/**
 * Files drive on macOS: every computer of the account as a location in the
 * Finder sidebar (plan step 23).
 *
 * The File Provider extension (Contents/PlugIns/AllternitFileProvider.appex,
 * built from the iOS extension sources and AllternitFilesKit) can't sign in
 * or list computers, so this manager does it and hands the result to the
 * extension through the helper `Contents/MacOS/allternit-files-domains`
 * (native/file-provider/Helper), which writes the shared App Group and adds
 * one File Provider domain per computer:
 *
 * 1. reads the account's `files_drive` flag (`GET /api/v1/me/flags`); off
 *    means every Files location is removed and nothing else happens;
 * 2. makes sure the extension has a Files token: a `compute`-scoped API key
 *    "Files on <this Mac>", minted again when the extension saw a 401;
 * 3. lists the computers (runtime devices through the relay, and the user's
 *    own mesh-only paired machines through this app's mesh proxies; this Mac
 *    and phones are left out) and syncs the domains;
 * 4. after the user switches Allternit on under System Settings > General >
 *    Login Items & Extensions > File Providers, re-adds each location once
 *    (spike 6: until then it stays empty) and watches for that switch while
 *    any location is off.
 *
 * The first-run checklist (design 3A) is renderer UI in allternit-ai; it
 * reads this manager through the `files-drive:*` IPC below.
 */
import { app, BrowserWindow, ipcMain, powerMonitor, shell } from 'electron';
import { spawn, execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import log from 'electron-log';
import { authManager } from './auth-manager.js';
import { URLS } from './config.js';
import { meshManager } from './mesh-manager.js';
import {
  type DomainStatus,
  type FilesComputer,
  type PairedComputerRow,
  type RuntimeDeviceRow,
  MESH_RUNTIME_PORT,
  makeDeviceId,
  pickComputers,
  planReenable,
} from './files-drive-logic.js';

/** What the helper prints (native/file-provider/Helper/main.swift `Status`). */
interface HelperStatus {
  appGroup: string;
  containerReady: boolean;
  hasToken: boolean;
  tokenRejected: boolean;
  tokenKeyId?: string;
  domains: DomainStatus[];
  added?: string[];
}

export type FilesDriveAvailability =
  /** Not macOS. */
  | 'unsupported'
  /** No extension in this build (dev run, or an ad-hoc signed build). */
  | 'missing'
  | 'available';

export interface FilesDriveStatus {
  availability: FilesDriveAvailability;
  signedIn: boolean;
  /** The account's `files_drive` flag; null until it was read once. */
  flag: boolean | null;
  computers: Array<{ id: string; name: string; enabled: boolean }>;
  /** Some location is off: the checklist asks for the File Providers switch. */
  needsProviderSwitch: boolean;
  syncing: boolean;
  lastSyncAt?: string;
  error?: string;
}

interface PersistedState {
  deviceId?: string;
  /** Computer id -> the location was last seen switched on (planReenable). */
  seen?: Record<string, boolean>;
}

const HELPER_NAME = 'allternit-files-domains';
const HELPER_TIMEOUT_MS = 60_000;
const SYNC_INTERVAL_MS = 10 * 60_000;
/** While the checklist is open: "This page checks every few seconds" (3A). */
const FAST_POLL_MS = 4_000;
const FAST_POLL_FOR_MS = 10 * 60_000;
/** While a location is off and nobody is looking. */
const SLOW_POLL_MS = 60_000;
const FOCUS_SYNC_MIN_MS = 60_000;

function cloudApiBaseUrl(): string {
  return (process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API).replace(/\/$/, '');
}

class FilesDriveManager {
  private status: FilesDriveStatus = {
    availability: process.platform === 'darwin' ? 'missing' : 'unsupported',
    signedIn: false,
    flag: null,
    computers: [],
    needsProviderSwitch: false,
    syncing: false,
  };
  private syncing: Promise<void> | null = null;
  private syncTimer: NodeJS.Timeout | null = null;
  private pollTimer: NodeJS.Timeout | null = null;
  private fastPollUntil = 0;
  private lastFocusSync = 0;
  private started = false;

  constructor() {
    ipcMain.handle('files-drive:status', () => this.status);
    ipcMain.handle('files-drive:sync', async () => {
      await this.sync();
      return this.status;
    });
    // The checklist calls this while it is on screen.
    ipcMain.handle('files-drive:watch', async () => {
      this.fastPollUntil = Date.now() + FAST_POLL_FOR_MS;
      await this.refreshStatus();
      this.schedulePoll();
      return this.status;
    });
    ipcMain.handle('files-drive:open-settings', () => this.openProviderSettings());
    ipcMain.handle('files-drive:show-in-finder', (_event, computerId: string) => this.showInFinder(computerId));
  }

  private get helperPath(): string {
    return path.join(path.dirname(process.execPath), HELPER_NAME);
  }

  private get statePath(): string {
    return path.join(app.getPath('userData'), 'files-drive.json');
  }

  /** Called once the app is up. Never throws; failures land in the status. */
  start(): void {
    if (this.started || process.platform !== 'darwin') return;
    this.started = true;
    if (!fs.existsSync(this.helperPath)) {
      log.info('[FilesDrive] No File Provider in this build (dev run or ad-hoc signed); Files in Finder is off.');
      return;
    }
    this.status.availability = 'available';
    void this.sync();
    this.syncTimer = setInterval(() => void this.sync(), SYNC_INTERVAL_MS);
    powerMonitor.on('resume', () => void this.sync());
    app.on('browser-window-focus', () => {
      if (this.status.needsProviderSwitch) {
        void this.refreshStatus();
      } else if ((!this.status.signedIn || this.status.flag === null) && Date.now() - this.lastFocusSync > FOCUS_SYNC_MIN_MS) {
        // Signed in (or back online) since the last try: don't wait 10 minutes.
        this.lastFocusSync = Date.now();
        void this.sync();
      }
    });
  }

  stop(): void {
    if (this.syncTimer) clearInterval(this.syncTimer);
    if (this.pollTimer) clearTimeout(this.pollTimer);
    this.syncTimer = null;
    this.pollTimer = null;
  }

  /** Sign-out: revoke the Files key and remove every location (and its local copies). */
  async signOut(): Promise<void> {
    if (this.status.availability !== 'available') return;
    await this.turnOff().catch((error) => log.warn('[FilesDrive] Sign-out cleanup failed:', error));
  }

  async sync(): Promise<void> {
    if (this.status.availability !== 'available') return;
    if (this.syncing) return this.syncing;
    this.syncing = this.runSync()
      .catch((error: unknown) => {
        this.status.error = error instanceof Error ? error.message : String(error);
        log.warn('[FilesDrive] Sync failed:', error);
      })
      .finally(() => {
        this.syncing = null;
        this.status.syncing = false;
        this.publish();
        this.schedulePoll();
      });
    return this.syncing;
  }

  // MARK: Sync

  private async runSync(): Promise<void> {
    this.status.syncing = true;
    this.publish();
    const session = await authManager.getSession();
    this.status.signedIn = !!session;
    if (!session) return;

    const flag = await this.readFlag();
    // Unreachable: keep whatever was set up last time.
    if (flag === null) return;
    this.status.flag = flag;
    if (!flag) {
      const current = await this.helper('status');
      if (current.domains.length > 0 || current.hasToken) await this.turnOff();
      this.applyStatus(await this.helper('status'));
      this.status.error = undefined;
      return;
    }

    const before = await this.helper('status');
    let token: { value: string; keyId?: string } | undefined;
    if (!before.hasToken || before.tokenRejected) {
      if (before.tokenKeyId) await this.revokeKey(before.tokenKeyId);
      token = await this.mintKey();
    }

    const [runtimes, paired] = await Promise.all([
      this.cloud<{ runtimes?: RuntimeDeviceRow[] }>('GET', '/api/v1/runtime-devices', { runtimeToken: true }),
      this.cloud<{ computers?: PairedComputerRow[] }>('GET', '/api/v1/computers/paired').catch((error) => {
        log.warn('[FilesDrive] Could not list paired computers (mesh-only machines left out):', error);
        return { computers: [] };
      }),
    ]);
    const { computers, meshOnly } = pickComputers(runtimes.runtimes ?? [], paired.computers ?? [], session.runtimeId);
    const meshProxies: Record<string, string> = {};
    for (const machine of meshOnly) {
      const target = `${machine.meshIp}:${MESH_RUNTIME_PORT}`;
      try {
        meshProxies[target] = await meshManager.loopbackFor(target);
      } catch (error) {
        log.warn(`[FilesDrive] No mesh proxy for ${target}; that computer stays unreachable from Finder:`, error);
      }
    }

    const state = this.readState();
    const deviceId = state.deviceId ?? makeDeviceId(randomBytes(16).toString('hex'));
    const synced = await this.helper('sync', {
      cloudBaseURL: cloudApiBaseUrl(),
      deviceId,
      deviceName: await this.computerName(),
      computers: computers satisfies FilesComputer[],
      token: token ? { value: token.value, keyId: token.keyId } : undefined,
      meshProxies,
    });
    const plan = planReenable(synced.domains, state.seen ?? {}, synced.added ?? []);
    this.writeState({ deviceId, seen: plan.seen });
    this.applyStatus(plan.reenable.length > 0 ? await this.helper('reenable', undefined, plan.reenable) : synced);
    this.status.lastSyncAt = new Date().toISOString();
    this.status.error = undefined;
  }

  /** Reads the helper's status and re-adds locations the user just switched on. */
  private async refreshStatus(): Promise<void> {
    if (this.status.availability !== 'available' || this.syncing || this.status.flag !== true) return;
    try {
      const current = await this.helper('status');
      const state = this.readState();
      const plan = planReenable(current.domains, state.seen ?? {});
      this.writeState({ ...state, seen: plan.seen });
      if (plan.reenable.length > 0) log.info(`[FilesDrive] File Provider switched on; re-adding ${plan.reenable.length} location(s).`);
      this.applyStatus(plan.reenable.length > 0 ? await this.helper('reenable', undefined, plan.reenable) : current);
    } catch (error) {
      log.warn('[FilesDrive] Status check failed:', error);
    }
    this.publish();
  }

  private schedulePoll(): void {
    if (this.pollTimer) clearTimeout(this.pollTimer);
    this.pollTimer = null;
    if (this.status.availability !== 'available' || this.status.flag !== true) return;
    const fast = Date.now() < this.fastPollUntil;
    if (!fast && !this.status.needsProviderSwitch) return;
    this.pollTimer = setTimeout(async () => {
      await this.refreshStatus();
      this.schedulePoll();
    }, fast ? FAST_POLL_MS : SLOW_POLL_MS);
  }

  private applyStatus(helper: HelperStatus): void {
    this.status.computers = helper.domains.map((d) => ({ id: d.computerId, name: d.name, enabled: d.userEnabled }));
    this.status.needsProviderSwitch = helper.domains.some((d) => !d.userEnabled);
  }

  private async turnOff(): Promise<void> {
    const current = await this.helper('status');
    if (current.tokenKeyId) await this.revokeKey(current.tokenKeyId);
    await this.helper('remove-all');
    this.writeState({ ...this.readState(), seen: {} });
    this.status.computers = [];
    this.status.needsProviderSwitch = false;
  }

  // MARK: Cloud

  /** `files_drive` from `GET /api/v1/me/flags`; null when it can't be read. */
  private async readFlag(): Promise<boolean | null> {
    if (process.env.ALLTERNIT_FILES_DRIVE === '1') return true;
    try {
      const body = await this.cloud<{ flags?: Record<string, unknown> }>('GET', '/api/v1/me/flags');
      return body.flags?.files_drive === true;
    } catch (error) {
      log.warn('[FilesDrive] Could not read the files_drive flag:', error);
      return null;
    }
  }

  private async mintKey(): Promise<{ value: string; keyId?: string }> {
    const name = `Files on ${await this.computerName()}`;
    const key = await this.cloud<{ id?: string; token?: string }>('POST', '/api/v1/api-keys', { body: { name, scopes: ['compute'] } });
    if (!key.token) throw new Error('The Files key response had no token.');
    return { value: key.token, keyId: key.id };
  }

  private async revokeKey(keyId: string): Promise<void> {
    await this.cloud('DELETE', `/api/v1/api-keys/${encodeURIComponent(keyId)}`).catch((error) => {
      log.warn('[FilesDrive] Could not revoke the old Files key:', error);
    });
  }

  /**
   * A cloud-api call as the signed-in user (Clerk), or with this Mac's
   * runtime device credential where that is what the route takes.
   */
  private async cloud<T>(method: string, route: string, options: { body?: unknown; runtimeToken?: boolean } = {}): Promise<T> {
    let bearer: string | null | undefined;
    if (options.runtimeToken) {
      bearer = (await authManager.getSession())?.accessToken;
    } else {
      bearer = await authManager.getClerkToken();
    }
    if (!bearer) throw new Error('Not signed in to Allternit Cloud.');
    const response = await fetch(`${cloudApiBaseUrl()}${route}`, {
      method,
      headers: {
        Authorization: `Bearer ${bearer}`,
        Accept: 'application/json',
        ...(options.body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
      signal: AbortSignal.timeout(30_000),
    });
    const text = await response.text();
    let payload: unknown = {};
    try {
      payload = text ? JSON.parse(text) : {};
    } catch {
      if (response.ok) throw new Error(`Allternit Cloud ${method} ${route} answered with something that is not JSON`);
    }
    if (!response.ok) {
      const message = (payload as { message?: string; error?: string }).message || (payload as { error?: string }).error;
      throw new Error(message || `Allternit Cloud ${method} ${route} failed (${response.status})`);
    }
    return payload as T;
  }

  // MARK: Helper

  private helper(command: 'status' | 'remove-all'): Promise<HelperStatus>;
  private helper(command: 'sync', input: unknown): Promise<HelperStatus>;
  private helper(command: 'reenable', input: undefined, args: string[]): Promise<HelperStatus>;
  private helper(command: string, input?: unknown, args: string[] = []): Promise<HelperStatus> {
    return this.runHelper<HelperStatus>([command, ...args], input);
  }

  private runHelper<T>(args: string[], input?: unknown): Promise<T> {
    return new Promise((resolve, reject) => {
      const child = spawn(this.helperPath, args, { stdio: ['pipe', 'pipe', 'pipe'] });
      let stdout = '';
      let stderr = '';
      const timer = setTimeout(() => child.kill('SIGKILL'), HELPER_TIMEOUT_MS);
      child.stdout.on('data', (chunk) => (stdout += chunk));
      child.stderr.on('data', (chunk) => (stderr += chunk));
      child.on('error', (error) => {
        clearTimeout(timer);
        reject(error);
      });
      child.on('close', (code) => {
        clearTimeout(timer);
        let parsed: unknown;
        try {
          parsed = JSON.parse(stdout.trim().split('\n').pop() || '{}');
        } catch {
          reject(new Error(`${HELPER_NAME} ${args[0]} printed no JSON (exit ${code}): ${stderr.trim().slice(0, 300)}`));
          return;
        }
        const error = (parsed as { error?: string }).error;
        if (code !== 0 || error) {
          reject(new Error(error || `${HELPER_NAME} ${args[0]} exited ${code}`));
          return;
        }
        resolve(parsed as T);
      });
      child.stdin.end(input === undefined ? '' : JSON.stringify(input));
    });
  }

  // MARK: UI actions

  /**
   * System Settings at the File Providers switch: Login Items & Extensions
   * on macOS 15 and later (Darwin 24+), the Extensions pane before that.
   */
  private async openProviderSettings(): Promise<{ opened: boolean }> {
    const darwinMajor = Number(os.release().split('.')[0]);
    const url = darwinMajor >= 24
      ? 'x-apple.systempreferences:com.apple.LoginItems-Settings.extension'
      : 'x-apple.systempreferences:com.apple.ExtensionsPreferences?extensionPointIdentifier=com.apple.fileprovider-nonui';
    try {
      await shell.openExternal(url);
      this.fastPollUntil = Date.now() + FAST_POLL_FOR_MS;
      this.schedulePoll();
      return { opened: true };
    } catch (error) {
      log.warn('[FilesDrive] Could not open System Settings:', error);
      return { opened: false };
    }
  }

  private async showInFinder(computerId: string): Promise<{ opened: boolean; error?: string }> {
    if (this.status.availability !== 'available') return { opened: false, error: 'Files in Finder is not in this build.' };
    try {
      const { url } = await this.runHelper<{ url: string }>(['url', String(computerId)]);
      const error = await shell.openPath(url);
      return error ? { opened: false, error } : { opened: true };
    } catch (error) {
      return { opened: false, error: error instanceof Error ? error.message : String(error) };
    }
  }

  // MARK: Local state

  /** The Mac's name as Sharing settings show it ("Joe's MacBook Pro"). */
  private computerName(): Promise<string> {
    return new Promise((resolve) => {
      execFile('/usr/sbin/scutil', ['--get', 'ComputerName'], { timeout: 3_000 }, (error, stdout) => {
        const name = !error ? stdout.trim() : '';
        resolve(name || os.hostname().replace(/\.local$/, '') || 'Mac');
      });
    });
  }

  private readState(): PersistedState {
    try {
      return JSON.parse(fs.readFileSync(this.statePath, 'utf8')) as PersistedState;
    } catch {
      return {};
    }
  }

  private writeState(state: PersistedState): void {
    try {
      fs.writeFileSync(this.statePath, JSON.stringify(state, null, 2) + '\n', { mode: 0o600 });
    } catch (error) {
      log.warn('[FilesDrive] Could not save state:', error);
    }
  }

  private publish(): void {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed()) window.webContents.send('files-drive:changed', this.status);
    }
  }
}

export const filesDriveManager = new FilesDriveManager();
