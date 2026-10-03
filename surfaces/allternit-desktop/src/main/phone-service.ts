/**
 * Wires the phone lane into Desktop: adb + device manager + tool runner +
 * approval dialog + loopback gateway + the MCP shim registration.
 * Everything with logic lives in the phone-*.ts modules and is unit tested;
 * this file is the Electron glue.
 */

import { app, dialog } from 'electron';
import log from 'electron-log';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { spawnSidecar } from './process-lifeline.js';
import { mcpHostManager } from './mcp-host-manager.js';
import { createAdbCall, ensureAdb, resolveAdb } from './phone-adb.js';
import { ArtemisManager } from './phone-artemis.js';
import { PhoneDeviceManager, type PhoneStore, type StoredPhone } from './phone-device-manager.js';
import { newGatewayToken, startPhoneGateway } from './phone-gateway.js';
import {
  PhoneToolRunner,
  type AdbCall,
  type ApprovalGate,
  type ApprovalRequest,
  type RateStore,
} from './phone-tools.js';

export const APPROVAL_TIMEOUT_MS = 120_000;

function readJson<T>(file: string, fallback: T): T {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8')) as T;
  } catch {
    return fallback;
  }
}

function writeJson(file: string, value: unknown, mode = 0o600): void {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(value, null, 2), { mode });
}

export function fileStore(file: string): PhoneStore {
  return { load: () => readJson<StoredPhone[]>(file, []), save: (phones) => writeJson(file, phones) };
}

export function fileRateStore(file: string): RateStore {
  return {
    load: (key) => readJson<Record<string, number[]>>(file, {})[key] ?? [],
    save: (key, timestamps) => writeJson(file, { ...readJson<Record<string, number[]>>(file, {}), [key]: timestamps }),
  };
}

/** Native confirm dialog: shows exactly what will be sent/dialed, defaults to Deny, expires like ACI grants (120s). */
export function dialogGate(): ApprovalGate {
  return {
    async request(req: ApprovalRequest) {
      const shown = dialog.showMessageBox({
        type: 'question',
        title: req.kind === 'sms' ? 'Allow this text?' : 'Allow this call?',
        message: req.kind === 'sms' ? 'Your bot wants to send a text from your phone' : 'Your bot wants to place a call from your phone',
        detail: req.summary,
        buttons: [req.kind === 'sms' ? 'Send' : 'Call', 'Deny'],
        defaultId: 1,
        cancelId: 1,
        noLink: true,
      });
      const timeout = new Promise<'timeout'>((resolve) => setTimeout(() => resolve('timeout'), APPROVAL_TIMEOUT_MS));
      const outcome = await Promise.race([shown, timeout]);
      if (outcome === 'timeout') return 'timeout';
      return outcome.response === 0 ? 'approved' : 'denied';
    },
  };
}

function findScrcpy(): string | null {
  const name = process.platform === 'win32' ? 'scrcpy.exe' : 'scrcpy';
  const candidates = [
    process.env.ALLTERNIT_SCRCPY_PATH,
    ...(process.env.PATH ?? '').split(path.delimiter).filter(Boolean).map((d) => path.join(d, name)),
    '/opt/homebrew/bin/scrcpy',
    '/usr/local/bin/scrcpy',
  ];
  return candidates.find((c) => c && fs.existsSync(c)) ?? null;
}

class PhoneService {
  private dataDir = '';
  private adbPath: string | null = null;
  private adb: AdbCall | null = null;
  private manager: PhoneDeviceManager | null = null;
  private artemis: ArtemisManager | null = null;
  private runner: PhoneToolRunner | null = null;
  private gatewayClose: (() => void) | null = null;
  private gatewayFile = path.join(os.homedir(), '.allternit', 'phone-gateway.json');

  getGatewayFile(): string {
    return this.gatewayFile;
  }

  /** Lazily brings adb up (downloading it on first use) and starts the keep-alive loop. */
  async ensureReady(): Promise<PhoneDeviceManager> {
    if (this.manager) return this.manager;
    this.dataDir = path.join(app.getPath('userData'), 'phone');
    fs.mkdirSync(this.dataDir, { recursive: true });
    this.adbPath = await ensureAdb(this.dataDir);
    this.adb = createAdbCall(this.adbPath);
    await this.adb(['start-server']);
    const scrcpy = findScrcpy();
    const manager = new PhoneDeviceManager({
      adb: this.adb,
      store: fileStore(path.join(this.dataDir, 'phones.json')),
      spawnScrcpy: scrcpy
        ? (serial, title) => spawnSidecar(scrcpy, ['-s', serial, '--window-title', title, '--stay-awake'], { stdio: 'ignore', windowsHide: true })
        : null,
    });
    this.artemis = new ArtemisManager(this.dataDir);
    const runner = new PhoneToolRunner({
      adb: this.adb,
      gate: dialogGate(),
      rates: fileRateStore(path.join(this.dataDir, 'rates.json')),
      artemis: () => (this.artemis?.status().state === 'ready' ? this.artemis : null),
      onlineSerials: () => manager.onlineSerials(),
      defaultSmsSubscription: (serial) => manager.defaultSmsSubscription(serial),
    });
    this.runner = runner;
    const token = newGatewayToken();
    const gateway = await startPhoneGateway({ token, runner });
    this.gatewayClose = gateway.close;
    writeJson(this.gatewayFile, { url: gateway.url, token });
    this.registerShim();
    manager.start();
    this.manager = manager;
    return manager;
  }

  /** Called at launch: only brings the lane up if a phone was ever paired (zero cost for everyone else). */
  async initIfPaired(): Promise<void> {
    const file = path.join(app.getPath('userData'), 'phone', 'phones.json');
    if (readJson<StoredPhone[]>(file, []).length === 0) return;
    await this.ensureReady().catch((error) => log.warn('[Phone] could not start:', error));
  }

  private registerShim(): void {
    const shim = app.isPackaged
      ? path.join(process.resourcesPath, 'phone', 'phone-mcp-shim.cjs')
      : path.resolve(app.getAppPath(), 'resources', 'phone', 'phone-mcp-shim.cjs');
    if (!fs.existsSync(shim)) {
      log.warn('[Phone] MCP shim missing at', shim);
      return;
    }
    mcpHostManager.registerRuntimeServer('allternit-phone', {
      command: process.execPath,
      args: [shim],
      env: { ELECTRON_RUN_AS_NODE: '1', ALLTERNIT_PHONE_GATEWAY_FILE: this.gatewayFile },
    });
  }

  adbStatus(): { path: string | null; ready: boolean } {
    const found = this.adbPath ?? resolveAdb(path.join(app.getPath('userData'), 'phone'));
    return { path: found, ready: this.manager !== null };
  }

  getArtemis(): ArtemisManager | null {
    return this.artemis;
  }

  /** Wizard "Try it": the one tool the renderer may call directly. Sends and dials stay bot-only, behind the approval dialog. */
  async tryOpenApp(serial: string, appName: string) {
    return this.runner?.call('phone.open_app', { serial, app: appName }) ?? { ok: false, error: 'device_offline', message: 'Phones are not set up yet.' };
  }

  async tryScreenshot(serial: string) {
    return this.runner?.call('phone.screenshot', { serial }) ?? { ok: false, error: 'device_offline', message: 'Phones are not set up yet.' };
  }

  shutdown(): void {
    this.manager?.stopAll();
    this.artemis?.stopAll();
    this.gatewayClose?.();
    fs.rmSync(this.gatewayFile, { force: true });
  }
}

export const phoneService = new PhoneService();
