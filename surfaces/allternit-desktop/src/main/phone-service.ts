/**
 * Wires the phone lanes into Desktop: the tool runner + approval dialog +
 * loopback gateway + MCP shim (shared), Android phones over adb, and iPhones
 * on an Allternit USB Agent. Everything with logic lives in the phone-*.ts
 * modules and is unit tested; this file is the Electron glue.
 *
 * The gateway comes up on its own: an iPhone-only user never downloads adb,
 * and adb is only fetched the first time an Android phone is used.
 */

import { app, dialog, nativeImage, safeStorage } from 'electron';
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
import { findUxplay } from './phone-airplay.js';
import { UsbAgentManager, type Sealer, type StoredUsbAgent, type UsbAgentConfig, type UsbAgentStore } from './phone-usb-agent.js';
import { candidatePorts, lanAddresses, serialPort } from './phone-usb-agent-provision.js';
import { USBA_MDNS_TYPE } from './phone-usb-agent-protocol.js';
import { loadFirmware, type BundledFirmware } from './phone-usb-agent-firmware.js';
import { grayFromBgra, type Gray } from './phone-usb-agent-vision.js';
import type { Calibration } from './phone-usb-agent-pointer.js';
import { IctrlClient, type SecretStore } from './ictrl-client.js';
import { fileLinkStore } from './phone-link-ios.js';
import { PhoneLinkBackend, cachedHostResolver, linkedPhoneSource, probeLinkedPhone, rememberLinkedHost, type LinkedPhone } from './phone-link-backend.js';
import { fileAndroidLinkStore } from './phone-link-android.js';
import { nodeDiscoveryDeps } from './phone-link-discovery.js';
import { PhoneLinkHealthMonitor } from './phone-link-health.js';
import type { PhoneLinkDevice } from './phone-link-types.js';
import { readSecret, writeSecret, deleteSecret } from './secure-store.js';
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

export function fileUsbAgentStore(dir: string): UsbAgentStore {
  const boards = path.join(dir, 'usb-agents.json');
  const config = path.join(dir, 'usb-agent-config.json');
  const calibration = (id: string) => path.join(dir, `usb-calibration-${id.replace(/[^a-z0-9-]/g, '')}.json`);
  return {
    load: () => readJson<StoredUsbAgent[]>(boards, []),
    save: (list) => writeJson(boards, list),
    loadConfig: () => readJson<UsbAgentConfig | null>(config, null),
    saveConfig: (c) => writeJson(config, c),
    loadCalibration: (id) => readJson<Calibration | null>(calibration(id), null),
    saveCalibration: (id, c) => (c ? writeJson(calibration(id), c) : fs.rmSync(calibration(id), { force: true })),
  };
}

/** Board keys sealed with the OS keychain; plain (0600 file) only where the OS offers no keychain. */
export function keychainSealer(): Sealer {
  return {
    seal: (hex) => (safeStorage.isEncryptionAvailable() ? `v1:${safeStorage.encryptString(hex).toString('base64')}` : `plain:${hex}`),
    open: (sealed) => {
      if (sealed.startsWith('v1:')) return safeStorage.decryptString(Buffer.from(sealed.slice(3), 'base64'));
      if (sealed.startsWith('plain:')) return sealed.slice(6);
      throw new Error('unknown key format');
    },
  };
}

/** JPEG → grayscale at about `width` px wide, with Chromium's decoder. */
function decodeGray(jpeg: Buffer, width: number): Gray | null {
  const img = nativeImage.createFromBuffer(jpeg);
  if (img.isEmpty()) return null;
  const size = img.getSize();
  const scaled = size.width > width ? img.resize({ width, quality: 'good' }) : img;
  const { width: w, height: h } = scaled.getSize();
  return grayFromBgra(w, h, new Uint8Array(scaled.toBitmap()));
}

/** Publishes the USB Agent link over mDNS so a board finds this computer after its IP changes. */
function advertiseLink(port: number, deskId: string) {
  const name = `Allternit ${deskId}`;
  if (process.platform === 'darwin') {
    return spawnSidecar('/usr/bin/dns-sd', ['-R', name, USBA_MDNS_TYPE, 'local', String(port), `id=${deskId}`], { stdio: 'ignore' });
  }
  const avahi = ['/usr/bin/avahi-publish-service', '/usr/local/bin/avahi-publish-service'].find((p) => fs.existsSync(p));
  return avahi ? spawnSidecar(avahi, [name, USBA_MDNS_TYPE, String(port), `id=${deskId}`], { stdio: 'ignore' }) : null;
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
  private adbReady: Promise<AdbCall> | null = null;
  private manager: PhoneDeviceManager | null = null;
  private managerReady: Promise<PhoneDeviceManager> | null = null;
  private usbAgents: UsbAgentManager | null = null;
  private artemis: ArtemisManager | null = null;
  private runner: PhoneToolRunner | null = null;
  private linkedBackend: PhoneLinkBackend | null = null;
  private gatewayReady: Promise<void> | null = null;
  private gatewayClose: (() => void) | null = null;
  private gatewayFile = path.join(os.homedir(), '.allternit', 'phone-gateway.json');

  getGatewayFile(): string {
    return this.gatewayFile;
  }

  private dir(): string {
    if (!this.dataDir) {
      this.dataDir = path.join(app.getPath('userData'), 'phone');
      fs.mkdirSync(this.dataDir, { recursive: true });
    }
    return this.dataDir;
  }

  /** adb, downloaded on first use. Also used by Phone Link setup. */
  ensureAdbCall(): Promise<AdbCall> {
    if (!this.adbReady) {
      this.adbReady = (async () => {
        this.adbPath = await ensureAdb(this.dir());
        const adb = createAdbCall(this.adbPath);
        await adb(['start-server']);
        return adb;
      })();
      this.adbReady.catch(() => {
        this.adbReady = null;
      });
    }
    return this.adbReady;
  }

  /**
   * Phone-Linked phones: one iCTRL lane over both link stores. Pure file
   * reads + HTTP clients — no adb download, no processes, so an iPhone-only
   * user pays nothing. Built once. A phone whose saved address went stale is
   * found again by a LAN scan for its device id (cachedHostResolver).
   */
  private ensureLinkedBackend(): PhoneLinkBackend {
    if (!this.linkedBackend) {
      const dir = this.dir();
      const secrets: SecretStore = { get: readSecret, set: writeSecret, delete: deleteSecret };
      const stores = {
        iosStore: fileLinkStore(path.join(dir, 'phone-link-ios.json')),
        androidStore: fileAndroidLinkStore(path.join(dir, 'phone-link-android.json')),
      };
      const source = linkedPhoneSource(stores);
      const resolveHost = cachedHostResolver({ discovery: nodeDiscoveryDeps, remember: rememberLinkedHost(stores) });
      const client = (phone: LinkedPhone, host: string) =>
        new IctrlClient({ host, secrets, storeKey: phone.storeKey, logger: (m) => log.info(m) });
      const health = new PhoneLinkHealthMonitor({
        phones: source,
        probe: (phone) => probeLinkedPhone(phone, client, resolveHost),
        logger: (m) => log.info(m),
      });
      this.linkedBackend = new PhoneLinkBackend({ phones: source, client, resolveHost, health, logger: (m) => log.info(m) });
    }
    return this.linkedBackend;
  }

  /** Linked phones with health, for phone-link:devices (Settings → Computers → Phones). */
  linkedDevices(): Promise<PhoneLinkDevice[]> {
    return this.ensureLinkedBackend().linkedDevices();
  }

  /** The tool runner, loopback gateway and MCP shim that every bot reaches phones through. */
  private ensureGateway(): Promise<void> {
    if (this.gatewayReady) return this.gatewayReady;
    this.gatewayReady = (async () => {
      const lazy = (async (args: string[], opts?: { serial?: string; timeoutMs?: number }) => (await this.ensureAdbCall())(args, opts)) as AdbCall;
      lazy.binary = async (args, opts) => (await this.ensureAdbCall()).binary(args, opts);
      this.artemis = this.artemis ?? new ArtemisManager(this.dir());
      const runner = new PhoneToolRunner({
        adb: lazy,
        gate: dialogGate(),
        rates: fileRateStore(path.join(this.dir(), 'rates.json')),
        artemis: () => (this.artemis?.status().state === 'ready' ? this.artemis : null),
        onlineSerials: () => this.manager?.onlineSerials() ?? [],
        defaultSmsSubscription: (serial) => this.manager?.defaultSmsSubscription(serial) ?? Promise.resolve(null),
        usb: () => this.usbAgents,
        links: () => this.ensureLinkedBackend(),
      });
      this.runner = runner;
      const token = newGatewayToken();
      const gateway = await startPhoneGateway({ token, runner });
      this.gatewayClose = gateway.close;
      writeJson(this.gatewayFile, { url: gateway.url, token });
      this.registerShim();
    })();
    this.gatewayReady.catch(() => {
      this.gatewayReady = null;
    });
    return this.gatewayReady;
  }

  /** Android lane: adb (downloading it on first use) and the keep-alive loop. */
  async ensureReady(): Promise<PhoneDeviceManager> {
    if (this.manager) return this.manager;
    if (!this.managerReady) {
      this.managerReady = (async () => {
        await this.ensureGateway();
        const adb = await this.ensureAdbCall();
        const scrcpy = findScrcpy();
        const manager = new PhoneDeviceManager({
          adb,
          store: fileStore(path.join(this.dir(), 'phones.json')),
          spawnScrcpy: scrcpy
            ? (serial, title) => spawnSidecar(scrcpy, ['-s', serial, '--window-title', title, '--stay-awake'], { stdio: 'ignore', windowsHide: true })
            : null,
        });
        manager.start();
        this.manager = manager;
        return manager;
      })();
      this.managerReady.catch(() => {
        this.managerReady = null;
      });
    }
    return this.managerReady;
  }

  /** iPhone lane: the USB Agent link, its mDNS advert and the AirPlay receivers. No adb involved. */
  async ensureUsbReady(): Promise<UsbAgentManager> {
    await this.ensureGateway();
    if (!this.usbAgents) {
      const uxplay = findUxplay();
      this.usbAgents = new UsbAgentManager({
        store: fileUsbAgentStore(this.dir()),
        sealer: keychainSealer(),
        spawnUxplay: uxplay ? (args) => spawnSidecar(uxplay, args, { stdio: ['ignore', 'ignore', 'ignore', 'ignore', 'pipe'] }) : null,
        advertise: process.platform === 'win32' ? null : advertiseLink,
        decode: decodeGray,
        registerFile: (slot) => path.join(this.dir(), `airplay-trusted-${slot}.txt`),
        openSerial: serialPort,
        listSerial: () => candidatePorts(),
        lanAddresses: () => lanAddresses(),
        bundledFirmware: () => this.firmwareVersion(),
      });
    }
    await this.usbAgents.start();
    return this.usbAgents;
  }

  /** True once the Android lane is up, or when an Android phone was ever paired (so status may start it). */
  androidInUse(): boolean {
    if (this.manager) return true;
    return readJson<StoredPhone[]>(path.join(app.getPath('userData'), 'phone', 'phones.json'), []).length > 0;
  }

  scrcpyInstalled(): boolean {
    return findScrcpy() !== null;
  }

  private firmware: BundledFirmware | null = null;

  /** The USB Agent firmware image this Desktop ships, verified against its manifest. */
  usbFirmware(): BundledFirmware {
    if (!this.firmware) {
      const dir = app.isPackaged
        ? path.join(process.resourcesPath, 'phone', 'usb-agent')
        : path.resolve(app.getAppPath(), 'resources', 'phone', 'usb-agent');
      this.firmware = loadFirmware(dir);
    }
    return this.firmware;
  }

  private firmwareVersion(): string | null {
    try {
      return this.usbFirmware().version;
    } catch {
      return null;
    }
  }

  getUsbAgents(): UsbAgentManager | null {
    return this.usbAgents;
  }

  /** Called at launch: only brings a lane up if something was ever paired on it (zero cost for everyone else). */
  async initIfPaired(): Promise<void> {
    const dir = path.join(app.getPath('userData'), 'phone');
    if (readJson<StoredPhone[]>(path.join(dir, 'phones.json'), []).length > 0) {
      await this.ensureReady().catch((error) => log.warn('[Phone] could not start:', error));
    }
    if (readJson<StoredUsbAgent[]>(path.join(dir, 'usb-agents.json'), []).length > 0) {
      await this.ensureUsbReady().catch((error) => log.warn('[Phone] USB Agent could not start:', error));
    }
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

  /** Wizard "Try it": the tools the renderer may call directly. Sends and dials stay bot-only, behind the approval dialog. */
  async tryOpenApp(serial: string, appName: string) {
    return this.runner?.call('phone.open_app', { serial, app: appName }) ?? { ok: false, error: 'device_offline', message: 'Phones are not set up yet.' };
  }

  async tryScreenshot(serial: string) {
    return this.runner?.call('phone.screenshot', { serial }) ?? { ok: false, error: 'device_offline', message: 'Phones are not set up yet.' };
  }

  async tryHome(serial: string) {
    return this.runner?.call('phone.home', { serial }) ?? { ok: false, error: 'device_offline', message: 'Phones are not set up yet.' };
  }

  shutdown(): void {
    this.manager?.stopAll();
    this.usbAgents?.stopAll();
    this.artemis?.stopAll();
    this.gatewayClose?.();
    fs.rmSync(this.gatewayFile, { force: true });
  }
}

export const phoneService = new PhoneService();
