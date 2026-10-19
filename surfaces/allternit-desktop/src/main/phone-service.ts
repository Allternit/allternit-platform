/**
 * Wires the phone lanes into Desktop: the tool runner + approval dialog +
 * loopback gateway + MCP shim (shared), Android phones over adb, and iPhones
 * on an Allternit USB Agent. Everything with logic lives in the phone-*.ts
 * modules and is unit tested; this file is the Electron glue.
 *
 * The gateway comes up on its own: an iPhone-only user never downloads adb,
 * and adb is only fetched the first time an Android phone is used.
 */

import { app, dialog, nativeImage, Notification, safeStorage } from 'electron';
import log from 'electron-log';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { spawnSidecar } from './process-lifeline.js';
import { mcpHostManager } from './mcp-host-manager.js';
import { createAdbCall, ensureAdb, resolveAdb } from './phone-adb.js';
import { ArtemisManager } from './phone-artemis.js';
import { PhoneDeviceManager, type StoredPhone } from './phone-device-manager.js';
import { fileStore, readJson, writeJson } from './phone-store-file.js';
import { newGatewayToken, startPhoneGateway, type PhoneGatewayHandover } from './phone-gateway.js';
import { execFileSync } from 'node:child_process';
import { handOver, handoverKey, handoverTargets, importHandover, openHandover, type HandoverClientDeps, type HandoverStores } from './phone-link-handover.js';
import { findUxplay } from './phone-airplay.js';
import {
  ensureScrcpyServer,
  nodeScrcpyAdb,
  nodeScrcpyConnect,
  ScrcpySession,
  type PhoneInputEvent,
  type PhoneStreamOptions,
  type PhoneStreamStatus,
} from './phone-scrcpy.js';
import { PhoneStreamManager, type StreamConsumer } from './phone-stream.js';
import { IosVideoSession } from './phone-ios-video.js';
import { PhoneUserComms, type CallStateName, type CommsResult, type PhoneAppInfo, type PhoneSmsMessage, type PhoneSmsThread } from './phone-user-comms.js';
import { PhoneLinkPhoneSocket, phoneLinkApiUrl } from './phone-link-cloud.js';
import { authManager } from './auth-manager.js';
import { backendManager } from './backend-manager.js';
import { URLS } from './config.js';
import { UsbAgentManager, type Sealer, type StoredUsbAgent, type UsbAgentConfig, type UsbAgentStore } from './phone-usb-agent.js';
import { candidatePorts, lanAddresses, serialPort } from './phone-usb-agent-provision.js';
import { USBA_MDNS_TYPE } from './phone-usb-agent-protocol.js';
import { loadFirmware, type BundledFirmware } from './phone-usb-agent-firmware.js';
import { grayFromBgra, type Gray } from './phone-usb-agent-vision.js';
import type { Calibration } from './phone-usb-agent-pointer.js';
import { IctrlClient, ictrlStoreKey, type SecretStore } from './ictrl-client.js';
import { fileLinkStore, findIosLinkRecord, setIosCallBridge, type IosLinkRecord } from './phone-link-ios.js';
import {
  androidCarrierLine,
  CallBridge,
  iosCarrierLine,
  requestLinkedCallRoom,
  type CallBridgeEvent,
  type CallBridgePhone,
  type CallBridgeStatus,
} from './phone-call-bridge.js';
import { CallBridgeWindow } from './phone-call-bridge-window.js';
import { callBridgeSummary, DEFAULT_CALL_BRIDGE, sanitizeCallBridge, type CallBridgeConfig } from './phone-call-bridge-config.js';
import { IosRecoveryScheduler, type IosRecoveryDeps } from './phone-ios-recovery.js';
import { createGoIosExec, createGoIosSpawn, ensureDdi, ensureGoIos, GoIos } from './phone-goios.js';
import { ensureRunnerArtifacts, pickSigner } from './phone-ios-runner.js';
import { readPairRecord, startLoopbackForward } from './phone-usbmux.js';
import { hasLinkedPhones } from './phone-link-stores.js';
import { catalogNameForPackage, humanizePackage } from './phone-app-catalog.js';
import { PhoneLinkBackend, cachedHostResolver, linkedPhoneSource, probeLinkedPhone, rememberLinkedApps,
  rememberLinkedHost, rememberLinkedMeshHost, LINK_SERIAL_PREFIX, type LinkedPhone } from './phone-link-backend.js';
import { createMeshHealer, phoneOffMeshRoutes, type MeshHealFn } from './phone-mesh-heal.js';
import { fileAndroidLinkStore, type AndroidLinkRecord } from './phone-link-android.js';
import { openHelperStream, streamLaneFor, relaySerials, resolveHelperStreamHost } from './phone-link-stream.js';
import { ScreenServiceKeeper, type AdbScreenPhone } from './phone-screen-service.js';
import { parseAdbDevices as parseAdbDeviceRows } from './phone-adb-pairing.js';
import { nodeDiscoveryDeps } from './phone-link-discovery.js';
import { PHONE_LINK_MESH_ENABLED } from './phone-link-mesh.js';
import { meshManager } from './mesh-manager.js';
import { PhoneLinkHealthMonitor } from './phone-link-health.js';
import { PhoneChannelReader, type ChannelConversation, type ReaderResult, type ScreenRecorder } from './phone-channel-readers.js';
import type { PhoneLinkDevice, PhoneLinkHandoverResult, PhoneLinkHandoverTarget } from './phone-link-types.js';
import { checkLinkedApp, openLinkedApp, type PhoneLinkAppResult } from './phone-app-signin.js';
import { PhoneAccountScheduler, readLinkedAccount, type PhoneLinkAccountResult } from './phone-account-reader.js';
import { readSecret, writeSecret, deleteSecret } from './secure-store.js';
import {
  PhoneToolRunner,
  type AdbCall,
  type ApprovalGate,
  type ApprovalRequest,
  type RateStore,
} from './phone-tools.js';

export const APPROVAL_TIMEOUT_MS = 120_000;

export { fileStore };

const LINK_DETACH_GRACE_MS = 30_000;
const LINK_SYNC_INTERVAL_MS = 60_000;

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
  private streams: PhoneStreamManager | null = null;
  private comms: PhoneUserComms | null = null;
  private linkSockets = new Map<string, PhoneLinkPhoneSocket>();
  private linkConsumers = new Map<string, () => void>();
  private linkDetachTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private linkSyncTimer: ReturnType<typeof setInterval> | null = null;
  private scrcpyServerReady: Promise<string> | null = null;
  private usbAgents: UsbAgentManager | null = null;
  private artemis: ArtemisManager | null = null;
  private runner: PhoneToolRunner | null = null;
  private linkedBackend: PhoneLinkBackend | null = null;
  private linkedHealth: PhoneLinkHealthMonitor | null = null;
  private iosRecovery: IosRecoveryScheduler | null = null;
  private gatewayReady: Promise<void> | null = null;
  private gatewayClose: (() => void) | null = null;
  private gatewayFile = path.join(os.homedir(), '.allternit', 'phone-gateway.json');
  private shimRegistered = false;
  private callBridge: CallBridge | null = null;
  private callBridgeWindow: CallBridgeWindow | null = null;
  private iosRecordsCache: { at: number; records: IosLinkRecord[] } | null = null;
  private androidRecordsCache: { at: number; records: AndroidLinkRecord[] } | null = null;
  private screenKeeper: ScreenServiceKeeper | null = null;
  private meshHealer: MeshHealFn | null = null;
  private accountScheduler: PhoneAccountScheduler | null = null;

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
   * found again by a LAN scan for its device id (cachedHostResolver), with a
   * mesh fallback behind the development switch (PHONE_LINK_MESH_ENABLED).
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
      // SECURITY: phones go on the mesh only behind the development switch;
      // without meshProxy the resolver never dials a mesh address.
      const resolveHost = cachedHostResolver({
        discovery: nodeDiscoveryDeps,
        remember: rememberLinkedHost(stores),
        ...(PHONE_LINK_MESH_ENABLED
          ? { meshProxy: (meshHost: string) => this.meshProxy(meshHost), healMesh: this.ensureMeshHealer() }
          : {}),
      });
      const client = (phone: LinkedPhone, host: string) =>
        new IctrlClient({ host, secrets, storeKey: phone.storeKey, logger: (m) => log.info(m) });
      const health = new PhoneLinkHealthMonitor({
        phones: source,
        probe: (phone) => probeLinkedPhone(phone, client, resolveHost),
        logger: (m) => log.info(m),
      });
      this.linkedHealth = health;
      this.linkedBackend = new PhoneLinkBackend({
        phones: source,
        client,
        resolveHost,
        health,
        saveApps: rememberLinkedApps(stores),
        logger: (m) => log.info(m),
      });
    }
    return this.linkedBackend;
  }

  /**
   * Phone mesh self-heal (phone-mesh-heal.ts): when a phone's mesh route
   * resets but the phone answers over the LAN or the cable, it is asked to
   * restart its mesh node (`mesh_restart`), at most once per 2 min per phone.
   * Shared by the host resolver, helper calls and the Android live screen.
   */
  private ensureMeshHealer(): MeshHealFn {
    if (!this.meshHealer) {
      const dir = this.dir();
      const secrets: SecretStore = { get: readSecret, set: writeSecret, delete: deleteSecret };
      const stores = {
        iosStore: fileLinkStore(path.join(dir, 'phone-link-ios.json')),
        androidStore: fileAndroidLinkStore(path.join(dir, 'phone-link-android.json')),
      };
      this.meshHealer = createMeshHealer({
        routes: phoneOffMeshRoutes({
          androidSerial: (deviceId) => this.androidRecords().find((r) => r.deviceId === deviceId)?.serial ?? null,
          iosUdid: (deviceId) => this.iosRecords().find((r) => (r.deviceId ?? r.udid) === deviceId)?.udid ?? null,
          // Only an adb that is already up: a heal never downloads or starts adb.
          adb: async () => (this.adbReady ? this.adbReady.catch(() => null) : null),
          ...(process.platform === 'darwin' ? { usbForward: (udid: string, port: number) => startLoopbackForward(udid, port) } : {}),
        }),
        fetchWhoami: nodeDiscoveryDeps.fetchWhoami,
        call: (phone, host, method, params, timeoutMs) =>
          new IctrlClient({ host, secrets, storeKey: phone.storeKey, logger: (m) => log.info(m) }).call(method, params, timeoutMs),
        rememberMeshHost: (phone, meshHost) => {
          rememberLinkedMeshHost(stores)(phone, meshHost);
          this.iosRecordsCache = null;
          this.androidRecordsCache = null;
        },
        log: (line) => log.info(line),
      });
    }
    return this.meshHealer;
  }

  /** A phone's mesh address (`<100.x>:8200`) bridged to a loopback host (`127.0.0.1:<port>`) by the mesh-node sidecar. */
  private async meshProxy(meshHost: string): Promise<string> {
    const url = await meshManager.proxyFor(`http://${meshHost}`);
    // The lane speaks to a loopback host, not a URL.
    return url.replace(/^https?:\/\//, '').replace(/\/$/, '');
  }

  /**
   * The iPhone recovery pass (phone-ios-recovery.ts): re-signs a linked
   * helper before the free-Apple-ID signature lapses, and remounts the
   * developer image + relaunches the helper when a restarted iPhone comes
   * back on the cable — zero clicks. Started at launch; each pass costs one
   * small JSON read when no iPhone is linked, so it stays free for everyone
   * else, and go-ios/the runner artifacts are only resolved after a link
   * record exists.
   */
  private ensureIosRecovery(): void {
    if (this.iosRecovery || process.platform !== 'darwin') return;
    if (this.iosRecords().length > 0) this.ensureLinkSync();
    const dir = this.dir();
    const store = fileLinkStore(path.join(dir, 'phone-link-ios.json'));
    let built: Promise<IosRecoveryDeps | null> | null = null;
    const deps = () => {
      if (store.all().length === 0) return Promise.resolve(null); // no linked iPhone: never touch go-ios
      built ??= (async (): Promise<IosRecoveryDeps | null> => {
        try {
          const bin = await ensureGoIos(dir);
          // go-ios keeps its tunnel identity (selfIdentity.plist) here; default would be the cwd.
          const tunnelDir = path.join(dir, 'go-ios', 'tunnel-identity');
          fs.mkdirSync(tunnelDir, { recursive: true, mode: 0o700 });
          const goios = new GoIos(createGoIosExec(bin), createGoIosSpawn(bin, tunnelDir), tunnelDir);
          const secrets: SecretStore = { get: readSecret, set: writeSecret, delete: deleteSecret };
          return {
            goios,
            artifacts: () => ensureRunnerArtifacts(dir),
            signer: pickSigner(app.isPackaged),
            ictrl: (udid, host) => new IctrlClient({ host, secrets, storeKey: ictrlStoreKey(udid), logger: (m) => log.info(m) }),
            forward: (udid, port) => startLoopbackForward(udid, port),
            ddiDir: async () => {
              const ddi = path.join(dir, 'ddi');
              await ensureDdi(ddi);
              return ddi;
            },
            readPairRecord: (udid) => readPairRecord(udid),
            store,
            workDir: path.join(dir, 'ictrl-work'),
            onRecovered: (udid, kind) => {
              // Health is evidence-based: a recovered phone is re-probed now,
              // so SIGNATURE_RENEWAL_REQUIRED / DEVELOPER_IMAGE_REMOUNT_REQUIRED
              // clear as soon as the helper truly answers again.
              const deviceId = store.load(udid)?.deviceId ?? udid;
              log.info(`[Phone] iPhone recovery (${kind}) succeeded for ${store.load(udid)?.deviceName ?? udid}; refreshing health`);
              void this.linkedHealth?.check(deviceId).catch(() => {});
            },
            logger: (m) => log.info(m),
          };
        } catch (error) {
          log.warn('[Phone] iPhone recovery tooling unavailable:', error);
          return null;
        }
      })();
      return built;
    };
    this.iosRecovery = new IosRecoveryScheduler({ deps, logger: (m) => log.info(m) });
    this.iosRecovery.start();
  }

  /** Linked phones with health, for phone-link:devices (Settings → Connections). */
  linkedDevices(): Promise<PhoneLinkDevice[]> {
    return this.ensureLinkedBackend().linkedDevices();
  }

  /** phone-link:open-app — open a catalog app on a linked phone for the Connections wizard. */
  openLinkedApp(opts: unknown): Promise<PhoneLinkAppResult> {
    return openLinkedApp({ lane: this.ensureLinkedBackend() }, opts);
  }

  /** phone-link:check-app — read a catalog app's screen on a linked phone and report whether it is signed in. */
  async checkLinkedApp(opts: unknown): Promise<PhoneLinkAppResult> {
    const result = await checkLinkedApp({ lane: this.ensureLinkedBackend() }, opts);
    // Just signed in on the phone: read its account, plan and usage soon.
    const o = (opts ?? {}) as { deviceId?: unknown; app?: unknown };
    if (result.ok && result.signedIn === true && typeof o.deviceId === 'string' && typeof o.app === 'string') {
      this.ensureAccountScheduler().kick(o.deviceId, o.app);
    }
    return result;
  }

  /** phone-link:read-account — read-only check of any app: signed in, account, and plan/usage where shown (phone-account-reader.ts). */
  readLinkedAccount(opts: unknown): Promise<PhoneLinkAccountResult> {
    const backend = this.ensureLinkedBackend();
    return readLinkedAccount({ lane: backend, busy: (deviceId) => this.linkedPhoneBusy(deviceId), logger: (m) => log.info(m) }, opts);
  }

  /** A bot is using the phone, or someone is watching or driving its live screen: background reads wait. */
  private linkedPhoneBusy(deviceId: string): boolean {
    return this.ensureLinkedBackend().botBusy(deviceId) || this.linkConsumers.has(LINK_SERIAL_PREFIX + deviceId);
  }

  /** Background checks: every app on each linked phone every 6 h, one app at a time (phone-account-reader.ts). */
  private ensureAccountScheduler(): PhoneAccountScheduler {
    if (!this.accountScheduler) {
      const backend = this.ensureLinkedBackend();
      this.accountScheduler = new PhoneAccountScheduler({
        phones: () => backend.linkedPhones(),
        health: (deviceId) => backend.healthOf(deviceId),
        busy: (deviceId) => this.linkedPhoneBusy(deviceId),
        read: (opts) => this.readLinkedAccount(opts),
        logger: (m) => log.info(m),
      });
      this.accountScheduler.start();
    }
    return this.accountScheduler;
  }

  // ── Pairing handover (phone-link-handover.ts) ──────────────────────────────

  private handoverStores(): HandoverStores {
    const dir = this.dir();
    return {
      iosStore: fileLinkStore(path.join(dir, 'phone-link-ios.json')),
      androidStore: fileAndroidLinkStore(path.join(dir, 'phone-link-android.json')),
      secrets: { get: readSecret, set: writeSecret, delete: deleteSecret },
    };
  }

  private signedInUser(): string | null {
    return authManager.getSessionSnapshot()?.userId ?? null;
  }

  /** Receiving side, served on the loopback phone gateway. */
  private handoverInbox(): PhoneGatewayHandover {
    return {
      key: () => handoverKey(this.handoverStores().secrets, this.signedInUser()),
      receive: async (sealed) => {
        const stores = this.handoverStores();
        const payload = openHandover(sealed, stores.secrets, this.signedInUser());
        const got = importHandover(stores, payload);
        log.info(`[Phone] received ${got.platform} phone ${got.deviceId} from ${payload.from.name} by pairing handover`);
        // Bots get the phone tools now, not at the next launch.
        void this.startForLinkedPhones();
        return got;
      },
    };
  }

  private handoverClient(): HandoverClientDeps {
    return {
      apiUrl: () => backendManager.getUrl(),
      headers: () => {
        const session = authManager.getSessionSnapshot();
        const base = backendManager.getLocalAuthHeaders(session?.userId ?? 'desktop-local');
        return session?.userEmail ? { ...base, 'x-allternit-user-email': session.userEmail } : base;
      },
      owner: () => this.signedInUser(),
      stores: this.handoverStores(),
      computerName: () => {
        if (process.platform === 'darwin') {
          try {
            const name = execFileSync('/usr/sbin/scutil', ['--get', 'ComputerName'], { encoding: 'utf8', timeout: 2000 }).trim();
            if (name) return name;
          } catch {
            // fall through to the host name
          }
        }
        return os.hostname().replace(/\.local$/, '');
      },
      confirm: async (phone, computerName) => {
        const kind = phone.platform === 'ios' ? 'iPhone' : 'phone';
        const result = await dialog.showMessageBox({
          type: 'question',
          buttons: [`Use on ${computerName}`, 'Cancel'],
          defaultId: 0,
          cancelId: 1,
          message: `Use ${phone.name} on ${computerName}?`,
          detail:
            `${computerName} gets this ${kind}'s pairing, so its apps work there while this computer is off. ` +
            'This computer keeps its pairing too.' +
            (phone.platform === 'ios'
              ? ' Renewing the iPhone helper each week and remounting after a restart still need this computer and a cable.'
              : ''),
        });
        return result.response === 0;
      },
    };
  }

  /**
   * phone-link:remove: forget a linked phone on this Desktop, its record and
   * its pairing secret. The helper app stays on the phone; linking again sets
   * it up fresh. False when no phone has that id.
   */
  removeLinked(deviceId: string): boolean {
    const dir = this.dir();
    const { secrets } = this.handoverStores();
    let removed = false;
    const drop = (file: string, matches: (key: string, record: { udid?: string; deviceId?: string }) => boolean, secretKey: (key: string) => string) => {
      let all: Record<string, { udid?: string; deviceId?: string }>;
      try {
        all = JSON.parse(fs.readFileSync(file, 'utf8'));
      } catch {
        return;
      }
      const keys = Object.keys(all).filter((key) => matches(key, all[key]));
      for (const key of keys) {
        delete all[key];
        secrets.delete(secretKey(key));
      }
      if (keys.length) fs.writeFileSync(file, JSON.stringify(all, null, 2), { mode: 0o600 });
      removed ||= keys.length > 0;
    };
    drop(path.join(dir, 'phone-link-ios.json'), (udid, r) => (r.deviceId ?? udid) === deviceId, (udid) => ictrlStoreKey(udid));
    drop(path.join(dir, 'phone-link-android.json'), (id) => id === deviceId, (id) => ictrlStoreKey(`android:${id}`));
    return removed;
  }

  /** phone-link:handover-targets — the person's bound Sessions computer, or none. */
  handoverTargets(): Promise<PhoneLinkHandoverTarget[]> {
    return handoverTargets(this.handoverClient());
  }

  /** phone-link:handover — confirm here, then seal and send one phone's pairing. */
  handover(opts: { deviceId: string; computerId: string }): Promise<PhoneLinkHandoverResult> {
    return handOver(this.handoverClient(), opts);
  }

  /** The tool runner, loopback gateway and MCP shim that every bot reaches phones through. */
  private async ensureGateway(): Promise<void> {
    await this.ensureGatewayServer();
    if (!this.shimRegistered) {
      this.shimRegistered = true;
      this.registerShim();
    }
  }

  /**
   * The loopback phone gateway alone, without giving bots the phone tools:
   * started at every launch so this computer can receive a phone handed over
   * from the person's Mac (phone-link-handover.ts) before it has a phone.
   */
  private ensureGatewayServer(): Promise<void> {
    if (this.gatewayReady) return this.gatewayReady;
    this.gatewayReady = (async () => {
      const lazy = (async (args: string[], opts?: { serial?: string; timeoutMs?: number }) => (await this.ensureAdbCall())(args, opts)) as AdbCall;
      lazy.binary = async (args, opts) => (await this.ensureAdbCall()).binary(args, opts);
      this.artemis = this.artemis ?? new ArtemisManager(this.dir(), undefined, (serial, running) =>
        this.manager?.noteAgentActivity(serial, running ? 1 : -1));
      const runner = new PhoneToolRunner({
        adb: lazy,
        gate: dialogGate(),
        rates: fileRateStore(path.join(this.dir(), 'rates.json')),
        artemis: () => (this.artemis?.status().state === 'ready' ? this.artemis : null),
        onlineSerials: () => this.manager?.onlineSerials() ?? [],
        defaultSmsSubscription: (serial) => this.manager?.defaultSmsSubscription(serial) ?? Promise.resolve(null),
        usb: () => this.usbAgents,
        activity: (serial, delta) => this.manager?.noteAgentActivity(serial, delta),
        links: () => this.ensureLinkedBackend(),
        onDialPlaced: (serial, number) => this.callBridge?.noteOutboundDial(serial, number) ?? false,
      });
      this.runner = runner;
      const token = newGatewayToken();
      const gateway = await startPhoneGateway({ token, runner, handover: this.handoverInbox() });
      this.gatewayClose = gateway.close;
      writeJson(this.gatewayFile, { url: gateway.url, token });
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
        manager.on('devices', () => {
          // The reconnect state machine handles re-pairing; a stream on a
          // phone that just went away simply ends and can be restarted.
          // Linked iPhones (`link:`) are not adb devices: their stream ends on its own socket.
          const online = new Set(manager.onlineSerials());
          for (const serial of this.streams?.activeSerials() ?? []) {
            if (!serial.startsWith(LINK_SERIAL_PREFIX) && !online.has(serial)) this.streams?.deviceDisconnected(serial);
          }
          this.syncLinkSockets();
          void this.screenKeeper?.pass();
        });
        this.manager = manager;
        this.ensureScreenKeeper();
        this.ensureStreamManager();
        this.ensureLinkSync();
        this.ensureCallBridge();
        return manager;
      })();
      this.managerReady.catch(() => {
        this.managerReady = null;
      });
    }
    return this.managerReady;
  }

  /** The pinned scrcpy-server binary; one shared download (verified) across sessions. */
  private ensureScrcpyServerPath(): Promise<string> {
    if (!this.scrcpyServerReady) {
      this.scrcpyServerReady = ensureScrcpyServer(this.dir());
      this.scrcpyServerReady.catch(() => {
        this.scrcpyServerReady = null; // a failed download may be retried on the next start
      });
    }
    return this.scrcpyServerReady;
  }

  /**
   * The in-app live view manager, one for both lanes: Android phones stream
   * over scrcpy-server (adb), linked iPhones (`link:<deviceId>`) over the
   * runner's /video (phone-ios-video.ts). Building it costs nothing — no adb,
   * no processes — so an iPhone-only user gets live video without adb.
   */
  private ensureStreamManager(): PhoneStreamManager {
    this.streams ??= new PhoneStreamManager({
      createSession: (serial, opts) => {
        const iphone = this.linkedIphone(serial);
        if (iphone) return this.createIosVideoSession(serial, iphone.name || 'iPhone');
        const android = this.linkedAndroid(serial);
        if (android) return this.createAndroidHelperSession(serial, android, opts);
        return new ScrcpySession(
          {
            adb: nodeScrcpyAdb(this.adbPath ?? '', serial),
            serial,
            serverPath: this.ensureScrcpyServerPath(),
            connect: nodeScrcpyConnect,
            log: (message) => log.info('[PhoneStream]', message),
          },
          opts,
        );
      },
    });
    return this.streams;
  }

  /** A `link:` serial that names a linked iPhone (phone-link-ios.json), else null. Never touches adb. */
  private linkedIphone(serial: string): LinkedPhone | null {
    if (!serial.startsWith(LINK_SERIAL_PREFIX) || this.iosRecords().length === 0) return null;
    const phone = this.ensureLinkedBackend().linkedPhone(serial);
    return streamLaneFor(serial, phone) === 'ios-video' ? phone : null;
  }

  /** A `link:` serial that names a linked Android phone (phone-link-android.json), else null. Never touches adb. */
  private linkedAndroid(serial: string): LinkedPhone | null {
    if (!serial.startsWith(LINK_SERIAL_PREFIX) || this.androidRecords().length === 0) return null;
    const phone = this.ensureLinkedBackend().linkedPhone(serial);
    return streamLaneFor(serial, phone) === 'android-helper' ? phone : null;
  }

  /**
   * A linked Android phone's live screen through its Phone Link app
   * (phone-link-stream.ts): scrcpy-server runs under the app's shell server
   * and each socket comes over the Allternit mesh — no adb, no Wireless
   * debugging, at home or away. The mesh host is resolved once per session.
   */
  private createAndroidHelperSession(serial: string, phone: LinkedPhone, opts: PhoneStreamOptions | undefined): ScrcpySession {
    const secrets: SecretStore = { get: readSecret, set: writeSecret, delete: deleteSecret };
    let route: Promise<{ host: string; client: IctrlClient }> | null = null;
    const resolveRoute = () => {
      route ??= (async () => {
        const host = await resolveHelperStreamHost(phone, {
          meshEnabled: PHONE_LINK_MESH_ENABLED,
          meshProxy: (meshHost) => this.meshProxy(meshHost),
          fetchWhoami: nodeDiscoveryDeps.fetchWhoami,
          healMesh: this.ensureMeshHealer(),
        });
        const client = new IctrlClient({ host, secrets, storeKey: phone.storeKey, logger: (m) => log.info(m) });
        // One sealed call first: proves the key and corrects the clock the envelope's ts is checked against.
        await client.call('device_info', {}, 10_000);
        return { host, client };
      })();
      route.catch(() => {
        route = null;
      });
      return route;
    };
    return new ScrcpySession(
      {
        serial,
        openStream: async (req) => {
          const { host, client } = await resolveRoute();
          return openHelperStream({ host, seal: (body) => client.sealBody(body) }, req);
        },
        log: (message) => log.info('[PhoneStream]', message),
      },
      opts,
    );
  }

  private createIosVideoSession(serial: string, deviceName: string): IosVideoSession {
    const backend = this.ensureLinkedBackend();
    return new IosVideoSession({
      call: (method, params, timeoutMs) => backend.helperCall(serial, method, params, timeoutMs),
      host: () => backend.hostFor(serial),
      deviceName,
      log: (message) => log.info('[PhoneStream]', message),
    });
  }

  /** The stream manager for Android callers: brings the adb lane up first. */
  async ensureStreams(): Promise<PhoneStreamManager> {
    await this.ensureReady();
    return this.ensureStreamManager();
  }

  /**
   * Start a live stream on request only. An Android phone must be online on
   * adb (the pinned scrcpy-server is downloaded on first use, not here); a
   * linked iPhone must be reachable on the iCTRL lane — no adb involved.
   */
  async startStream(serial: string, opts?: PhoneStreamOptions): Promise<{ ok: true } | { ok: false; message: string }> {
    if (serial.startsWith(LINK_SERIAL_PREFIX)) {
      // A phone linked a moment ago must be found.
      this.iosRecordsCache = null;
      this.androidRecordsCache = null;
      if (this.linkedAndroid(serial)) {
        // Linked Android: through Phone Link over the mesh (no adb lane needed).
        if (!this.ensureLinkedBackend().has(serial)) {
          return { ok: false, message: "Can't reach that phone right now. Check it is on and Allternit Phone Link is running on it." };
        }
        return this.ensureStreamManager().start(serial, opts ?? {});
      }
      if (!this.linkedIphone(serial)) return { ok: false, message: 'That phone is not linked to this computer.' };
      if (!this.ensureLinkedBackend().has(serial)) {
        return { ok: false, message: "Can't reach that iPhone right now. Check it is unlocked and on the same Wi-Fi as this computer." };
      }
      return this.ensureStreamManager().start(serial, opts ?? {});
    }
    const manager = await this.ensureReady();
    const phone = manager.list().find((p) => p.serial === serial);
    if (!phone || phone.state !== 'online') return { ok: false, message: 'That phone is not connected.' };
    return this.ensureStreams().then((streams) => streams.start(serial, opts ?? {}));
  }

  /** Stream controls, safe to call before the Android lane exists (no-op / idle results). */
  async stopStream(serial: string): Promise<void> {
    await this.streams?.stop(serial);
  }

  streamStatus(serial: string): PhoneStreamStatus {
    return this.streams?.status(serial) ?? { state: 'idle' };
  }

  sendStreamInput(serial: string, events: unknown): { ok: boolean; message?: string } {
    if (!Array.isArray(events) || events.length === 0) return { ok: false, message: 'No input events.' };
    // encodeControlMessage validates every event; invalid ones fail as { ok: false }.
    return this.streams?.sendInput(serial, events as PhoneInputEvent[]) ?? { ok: false, message: 'The stream is not live.' };
  }

  setStreamDisplayPower(serial: string, on: boolean): { ok: boolean; message?: string } {
    return this.streams?.setDisplayPower(serial, on) ?? { ok: false, message: 'The stream is not live.' };
  }

  // ── User comms (Phones 2.0 Phase 3): texts and calls through the phone's   ──
  // ── own SIM. The bridge methods take no serial — this lane is "my phone":  ──
  // ── main resolves the linked, online phone. The bot tool lane keeps its    ──
  // ── own approval-gated path; these methods have no dialog (the user acts). ──

  async ensureComms(): Promise<PhoneUserComms> {
    await this.ensureReady();
    if (!this.comms) {
      const lazy = (async (args: string[], opts?: { serial?: string; timeoutMs?: number }) => (await this.ensureAdbCall())(args, opts)) as AdbCall;
      lazy.binary = async (args, opts) => (await this.ensureAdbCall()).binary(args, opts);
      this.comms = new PhoneUserComms({
        adb: lazy,
        rates: fileRateStore(path.join(this.dir(), 'rates.json')),
        onlineSerials: () => this.manager?.onlineSerials() ?? [],
        log: (message) => log.info('[PhoneComms]', message),
      });
    }
    return this.comms;
  }

  /** The phone the user comms lane acts on; `ok:false` when nothing is connected. */
  private async commsTarget(): Promise<{ serial: string } | { ok: false; message: string }> {
    const manager = await this.ensureReady();
    const online = manager.onlineSerials();
    if (online.length === 0) return { ok: false as const, message: 'No phone is connected.' };
    return { serial: online[0] };
  }

  smsList(): Promise<CommsResult<{ threads: PhoneSmsThread[] }>> {
    return this.commsWith(async (serial, comms) => comms.smsList(serial));
  }

  smsMessages(address: string, opts?: { afterTimestamp?: number }): Promise<CommsResult<{ messages: PhoneSmsMessage[] }>> {
    return this.commsWith(async (serial, comms) => comms.smsMessages(serial, address, opts));
  }

  smsSendUser(to: string, text: string): Promise<CommsResult<{ status: 'submitted'; emergency?: true }>> {
    const body = String(text ?? '');
    if (!body.trim()) return Promise.resolve({ ok: false as const, message: 'Empty message.' });
    if (body.length > 1600) return Promise.resolve({ ok: false as const, message: 'Message is too long (1600 characters max).' });
    return this.commsWith(async (serial, comms) => comms.smsSendUser(serial, to, body));
  }

  callDialUser(number: string, opts?: { confirmEmergency?: boolean }): Promise<CommsResult<{ status: 'dialing'; emergency?: true }>> {
    return this.commsWith(async (serial, comms) => comms.callDialUser(serial, number, opts));
  }

  callEnd(): Promise<CommsResult<{}>> {
    return this.commsWith(async (serial, comms) => comms.callEnd(serial));
  }

  /** The bridge shape has no `ok` — a failure or no phone reads as idle. */
  async callState(): Promise<{ state: CallStateName; number?: string }> {
    const target = await this.commsTarget();
    if ('ok' in target) return { state: 'idle' };
    const result = await (await this.ensureComms()).callState(target.serial);
    return result.ok ? { state: result.state, ...(result.number !== undefined ? { number: result.number } : {}) } : { state: 'idle' };
  }

  /**
   * The PhoneViewport apps sheet inventory: `{packageName, label}` per app.
   * Names come from Phone Link (the helper knows each app's label); over
   * adb alone Android gives only package names, so those rows use the
   * catalog's name or the package's last word, and carrier stubs are dropped.
   */
  async listApps(serial: string): Promise<CommsResult<{ apps: Array<{ packageName: string; label: string }> }>> {
    const byLabel = (apps: Array<{ packageName: string; label: string }>) => [...apps].sort((x, y) => x.label.localeCompare(y.label));
    const link = this.ensureLinkedBackend();
    const linkSerial = serial.startsWith(LINK_SERIAL_PREFIX) ? serial : await this.linkSerialFor(serial);
    if (linkSerial) {
      const named = await link.installedApps(linkSerial).catch(() => null);
      if (named?.length) return { ok: true, apps: byLabel(named) };
      if (serial.startsWith(LINK_SERIAL_PREFIX)) return { ok: false as const, message: 'Couldn’t list the phone’s apps. Check that Phone Link is running on it.' };
    }
    const manager = await this.ensureReady();
    const phone = manager.list().find((p) => p.serial === serial);
    if (!phone || phone.state !== 'online') return { ok: false as const, message: 'That phone is not connected.' };
    const res = await (await this.ensureComms()).listApps(serial);
    if (!res.ok) return res;
    return {
      ok: true,
      apps: byLabel(res.apps
        .filter((a) => !/^stub/i.test(a.versionName ?? ''))
        .map((a) => ({ packageName: a.pkg, label: catalogNameForPackage(a.pkg) ?? humanizePackage(a.pkg) }))),
    };
  }

  /** The Phone Link serial of the same phone as an adb serial (matched by name), if it is linked and healthy. */
  private async linkSerialFor(adbSerial: string): Promise<string | null> {
    const name = this.manager?.list().find((p) => p.serial === adbSerial)?.name?.trim().toLowerCase();
    const android = (await this.linkedDevices().catch(() => [] as PhoneLinkDevice[]))
      .filter((d) => d.platform === 'android' && d.health.state === 'HEALTHY');
    const match = android.find((d) => name && d.name?.trim().toLowerCase() === name) ?? (android.length === 1 ? android[0] : undefined);
    return match ? LINK_SERIAL_PREFIX + match.deviceId : null;
  }

  getComms(): PhoneUserComms | null {
    return this.comms;
  }

  private async commsWith<T>(fn: (serial: string, comms: PhoneUserComms) => Promise<CommsResult<T>>): Promise<CommsResult<T>> {
    const target = await this.commsTarget();
    if ('ok' in target) return target;
    return fn(target.serial, await this.ensureComms());
  }

  /** Always available (no adb needed): renderers subscribe before either lane is up. */
  getStreamManager(): PhoneStreamManager {
    return this.ensureStreamManager();
  }

  // ── Cloud relay (Phones 2.0 Phase 2): the persistent session that lets     ──
  // ── signed-in surfaces reach this phone through allternit-api.             ──

  /** The session connects only once a phone is paired; it idles otherwise. */
  // ── Cloud relay (Phones 2.0 Phase 2): one phone-link socket per linked     ──
  // ── phone, so a viewer never waits behind another phone's stream (the      ──
  // ── relay streams one serial per socket — see phone_ws.rs). The relay      ──
  // ── signals viewers with viewer_attached/detached; scrcpy starts on        ──
  // ── attach and idles off on a grace after the last detach.                 ──

  /** Signed out ⇒ no sockets; the sync timer picks sign-in up within a minute. */
  /** Keep one relay socket per phone a web viewer can open; started by the Android lane or a linked iPhone. */
  private ensureLinkSync(): void {
    if (this.linkSyncTimer) return;
    this.linkSyncTimer = setInterval(() => this.syncLinkSockets(), LINK_SYNC_INTERVAL_MS);
    this.linkSyncTimer.unref?.();
    this.syncLinkSockets();
  }

  private syncLinkSockets(): void {
    if (!authManager.getSessionSnapshot()) return;
    // Linked iPhones stream their live screen (IosVideoSession) and linked
    // Androids theirs through Phone Link (helper mode), so the web and PWA can
    // open them too; a linked Android stands in for its adb twin while that
    // twin is offline.
    const androidLinks = this.androidRecords();
    const backend = androidLinks.length ? this.ensureLinkedBackend() : null;
    const serials = relaySerials({
      adb: (this.manager?.list() ?? []).map((phone) => ({ serial: phone.serial, id: phone.id, state: phone.state })),
      iosDeviceIds: this.iosRecords().map((r) => r.deviceId ?? r.udid),
      androidLinks: androidLinks.filter((r) => r.meshHost).map((r) => ({ deviceId: r.deviceId, serial: r.serial })),
      linkReachable: (deviceId) => backend?.has(`${LINK_SERIAL_PREFIX}${deviceId}`) ?? false,
    });
    const seen = new Set<string>();
    for (const serial of serials) {
      seen.add(serial);
      if (!this.linkSockets.has(serial)) {
        this.linkSockets.set(serial, this.createLinkSocket(serial));
      }
      this.linkSockets.get(serial)?.notePhoneChanged();
    }
    for (const [serial, socket] of this.linkSockets) {
      if (seen.has(serial)) continue;
      socket.stop();
      this.linkSockets.delete(serial);
      this.cancelLinkDetach(serial);
      this.detachLinkConsumer(serial);
    }
  }

  private createLinkSocket(serial: string): PhoneLinkPhoneSocket {
    const client = new PhoneLinkPhoneSocket({
      // The relay is this Desktop's own allternit-api: api.allternit.com is
      // the cloud control plane and has no /ws/phone-link route (404), and the
      // viewer's stream token is minted by this api. Web viewers reach it via
      // the cloud runtime socket relay. See phoneLinkApiUrl.
      apiUrl: () => phoneLinkApiUrl(backendManager.getUrl()),
      // The desktop-access-token credential + the signed-in user's headers —
      // the api's auth middleware maps them to the account browsers mint
      // tokens against (auth.rs verify_desktop_access_token + user headers).
      headers: () => {
        const base = backendManager.getLocalAuthHeaders();
        const session = authManager.getSessionSnapshot();
        const userId = session?.userId ?? 'desktop-local';
        return {
          ...base,
          'x-allternit-user-id': userId,
          'x-allternit-user-email': session?.userEmail ?? `${userId}@desktop.allternit.local`,
        };
      },
      phone: () => {
        const iphone = this.linkedIphone(serial);
        if (iphone) return { serial, model: 'iPhone', name: iphone.name || 'iPhone' };
        const android = this.linkedAndroid(serial);
        if (android) return { serial, model: 'Android', name: android.name || 'Android phone' };
        const p = this.manager?.list().find((entry) => entry.serial === serial);
        const bridge = p ? this.manager?.getCallBridge(p.id) : null;
        return {
          serial,
          model: p?.model ?? '',
          name: p?.name || p?.model || 'Linked phone',
          // Web/PWA show the Call audio state read-only; device ids and labels stay on this Mac.
          ...(bridge ? { callBridge: callBridgeSummary(bridge) } : {}),
        };
      },
      onViewerAttached: (s) => this.linkViewerAttached(s),
      onViewerDetached: (s) => this.linkViewerDetached(s),
      onInput: (s, events) => this.sendStreamInput(s, events),
      onDisplayPower: (s, on) => this.setStreamDisplayPower(s, on),
      log: (message) => log.info(message),
    });
    client.start();
    return client;
  }

  /** A viewer appeared on the relay: start scrcpy and forward its feed. */
  private async linkViewerAttached(serial: string): Promise<{ ok: boolean; message?: string }> {
    this.cancelLinkDetach(serial);
    const socket = this.linkSockets.get(serial);
    if (!socket) return { ok: false, message: 'The relay session is gone.' };
    const started = await this.startStream(serial);
    if (!started.ok) return started;
    if (!this.linkConsumers.has(serial)) {
      // Linked phones (`link:`) stream without the adb lane.
      const streams = serial.startsWith(LINK_SERIAL_PREFIX) ? this.ensureStreamManager() : await this.ensureStreams();
      const consumer: StreamConsumer = {
        onMeta: (_s, meta) => socket.push({ kind: 'meta', meta }),
        onChunk: (_s, chunk) => socket.push({ kind: 'chunk', chunk }),
        // Replayed on subscribe (the stream is already live here); the socket
        // sends audio only to viewers that opted in with {type:'audio', on:true}.
        onAudioMeta: (_s, meta) => socket.push({ kind: 'audioMeta', meta }),
        onAudioChunk: (_s, chunk) => socket.push({ kind: 'audioChunk', chunk }),
        onEnd: (_s, info) => {
          socket.push({ kind: 'end', reason: info.reason });
          this.detachLinkConsumer(serial);
        },
      };
      this.linkConsumers.set(serial, streams.subscribe(serial, consumer));
    }
    return { ok: true };
  }

  /** Last viewer left: grace before tearing scrcpy down so a quick reconnect is seamless. */
  private linkViewerDetached(serial: string): void {
    this.cancelLinkDetach(serial);
    const timer = setTimeout(() => {
      this.linkDetachTimers.delete(serial);
      void this.stopStream(serial);
      this.detachLinkConsumer(serial);
    }, LINK_DETACH_GRACE_MS);
    timer.unref?.();
    this.linkDetachTimers.set(serial, timer);
  }

  private cancelLinkDetach(serial: string): void {
    const timer = this.linkDetachTimers.get(serial);
    if (timer) clearTimeout(timer);
    this.linkDetachTimers.delete(serial);
  }

  private detachLinkConsumer(serial: string): void {
    this.linkConsumers.get(serial)?.();
    this.linkConsumers.delete(serial);
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

  /**
   * Bots reach Phone-Linked phones through the gateway + MCP shim. Called at
   * launch when a linked phone exists and when Phone Link setup finishes, so
   * the phone tools are there without an adb phone or USB Agent. No adb
   * download, no processes beyond the gateway.
   */
  async startForLinkedPhones(): Promise<void> {
    await this.ensureGateway().catch((error) => log.warn('[Phone] gateway for linked phones could not start:', error));
    this.iosRecordsCache = null;
    this.androidRecordsCache = null;
    this.ensureCallBridge();
    // Relay sockets for linked phones (iPhone video, Android through Phone Link) without the adb lane.
    this.ensureLinkSync();
    if (this.androidRecords().length > 0) this.ensureScreenKeeper();
    // Account, plan and usage for signed-in AI chat apps: right after linking, then every 6 h.
    this.ensureAccountScheduler().kick();
  }

  /**
   * Linked Androids' screen service (phone-screen-service.ts): whenever one is
   * online over adb (USB or home Wireless debugging), Desktop starts the Phone
   * Link app's shell server through that trusted connection, so its live
   * screen keeps working over the mesh after it leaves home (until it restarts).
   */
  private ensureScreenKeeper(): void {
    if (this.screenKeeper) return;
    this.screenKeeper = new ScreenServiceKeeper({
      links: () => this.androidRecords().map((r) => ({ deviceId: r.deviceId, serial: r.serial, deviceName: r.deviceName })),
      adbPhones: async () => {
        const adb = await this.ensureAdbCall();
        const rows = parseAdbDeviceRows((await adb(['devices', '-l'], { timeoutMs: 10_000 })).stdout).filter((r) => r.state === 'device');
        const known = this.manager?.list() ?? [];
        return rows.map((row): AdbScreenPhone => {
          const stored = known.find((p) => p.serial === row.serial);
          return { serial: row.serial, hardwareSerial: stored?.id ?? null, model: stored?.model ?? row.model ?? null, name: stored?.name ?? null };
        });
      },
      readHardwareSerial: async (serial) => {
        const adb = await this.ensureAdbCall();
        const out = (await adb(['shell', 'getprop', 'ro.serialno'], { serial, timeoutMs: 10_000 })).stdout.trim();
        return out || null;
      },
      helperCall: (deviceId, method, params, timeoutMs) =>
        this.ensureLinkedBackend().helperCall(`${LINK_SERIAL_PREFIX}${deviceId}`, method, params, timeoutMs),
      // One short-lived `adb -s <serial> shell <line>` per line; the lines are validated first.
      adbShell: async (serial, line) => (await this.ensureAdbCall())(['shell', line], { serial, timeoutMs: 15_000 }),
      log: (message) => log.info(message),
    });
    this.screenKeeper.start();
  }

  // ── Call bridge (CONTRACT-linked-phone-voice C5): the bot talks on a real  ──
  // ── carrier call through an audio cable to this Mac. Logic and states live ──
  // ── in phone-call-bridge.ts; this is the Electron glue.                    ──

  /** The watcher: started with the Android lane or when a linked phone exists; idle phones cost nothing. */
  ensureCallBridge(): CallBridge {
    if (this.callBridge) return this.callBridge;
    const media = new CallBridgeWindow({
      onDisconnected: (serial, reason) => (serial === '*' ? bridge.mediaLost(reason) : bridge.mediaDisconnected(serial, reason)),
      log: (m) => log.info(m),
    });
    const bridge: CallBridge = new CallBridge({
      phones: () => this.callBridgePhones(),
      carrier: {
        android: androidCarrierLine(() => this.ensureComms()),
        ios: iosCarrierLine((phone, method, params) => this.ensureLinkedBackend().helperCall(phone.serial, method, params ?? {})),
      },
      cloud: {
        createRoom: (req) =>
          requestLinkedCallRoom(
            {
              baseUrl: () => process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API,
              deviceToken: () => authManager.getSessionSnapshot()?.accessToken || null,
              clerkToken: () => authManager.getClerkToken(5_000),
              fetch: ((input, init) => fetch(input, init)) as typeof fetch,
            },
            req,
          ),
      },
      media,
      log: (m) => log.info(m),
    });
    bridge.on('event', (ev: CallBridgeEvent) => {
      // The needs-you notice: shown even when no Allternit window is open.
      if (ev.type === 'notice') {
        if (Notification.isSupported()) new Notification({ title: ev.title, body: ev.message }).show();
        log.warn(`[CallBridge] ${ev.title}`);
      }
    });
    bridge.start();
    this.callBridge = bridge;
    this.callBridgeWindow = media;
    return bridge;
  }

  getCallBridge(): CallBridge | null {
    return this.callBridge;
  }

  /** Linked iPhone records, re-read at most every 5 s (the watcher asks every second). */
  private iosRecords(): IosLinkRecord[] {
    const now = Date.now();
    if (!this.iosRecordsCache || now - this.iosRecordsCache.at > 5_000) {
      this.iosRecordsCache = { at: now, records: fileLinkStore(path.join(this.dir(), 'phone-link-ios.json')).all() };
    }
    return this.iosRecordsCache.records;
  }

  /** Linked Android records (Phone Link app), re-read at most every 5 s. */
  private androidRecords(): AndroidLinkRecord[] {
    const now = Date.now();
    if (!this.androidRecordsCache || now - this.androidRecordsCache.at > 5_000) {
      this.androidRecordsCache = { at: now, records: fileAndroidLinkStore(path.join(this.dir(), 'phone-link-android.json')).all() };
    }
    return this.androidRecordsCache.records;
  }

  /** Every phone that can carry a bridged call: Android over adb, iPhones on the iCTRL helper. */
  private callBridgePhones(): CallBridgePhone[] {
    const out: CallBridgePhone[] = [];
    const manager = this.manager;
    for (const p of manager?.list() ?? []) {
      out.push({
        serial: p.serial,
        deviceId: p.id,
        label: p.name || p.model || 'Android phone',
        platform: 'android',
        online: p.state === 'online',
        config: manager?.getCallBridge(p.id) ?? null,
      });
    }
    const ios = this.iosRecords();
    if (ios.length) {
      const backend = this.ensureLinkedBackend();
      for (const r of ios) {
        const serial = `link:${r.deviceId ?? r.udid}`;
        out.push({ serial, deviceId: r.udid, label: r.deviceName || 'iPhone', platform: 'ios', online: backend.has(serial), config: r.callBridge ?? null });
      }
    }
    return out;
  }

  private async callBridgeLanes(): Promise<void> {
    if (!this.manager && this.androidInUse()) await this.ensureReady().catch(() => {});
    this.ensureCallBridge();
  }

  /** phone:call-bridge-get — the phone's Call audio settings (defaults when never set) and its live status. */
  async callBridgeGet(serial: string): Promise<{ ok: true; config: CallBridgeConfig; status: CallBridgeStatus | null } | { ok: false; message: string }> {
    await this.callBridgeLanes();
    const android = this.manager?.list().find((p) => p.serial === serial || p.id === serial);
    if (android) {
      return { ok: true, config: this.manager?.getCallBridge(android.id) ?? { ...DEFAULT_CALL_BRIDGE }, status: this.callBridge?.statusFor(android.serial) ?? null };
    }
    const ios = findIosLinkRecord(fileLinkStore(path.join(this.dir(), 'phone-link-ios.json')), serial);
    if (ios) {
      return { ok: true, config: ios.callBridge ?? { ...DEFAULT_CALL_BRIDGE }, status: this.callBridge?.statusFor(`link:${ios.deviceId ?? ios.udid}`) ?? null };
    }
    return { ok: false, message: this.callBridgeUnknown(serial) };
  }

  /** phone:call-bridge-set — save Call audio on the phone's record; turning it on asks for the microphone once and checks the cable. */
  async callBridgeSet(serial: string, raw: unknown): Promise<{ ok: true; config: CallBridgeConfig; status: CallBridgeStatus | null } | { ok: false; message: string }> {
    await this.callBridgeLanes();
    const cfg = sanitizeCallBridge(raw);
    let key: string | null = null;
    const android = this.manager?.list().find((p) => p.serial === serial || p.id === serial);
    if (android && this.manager?.setCallBridge(android.id, cfg)) {
      key = android.serial;
      // Re-hello now so web/PWA see the new Call audio state (setCallBridge's 'devices' event does too, debounced).
      this.linkSockets.get(android.serial)?.notePhoneChanged();
    }
    if (!key) {
      const store = fileLinkStore(path.join(this.dir(), 'phone-link-ios.json'));
      const ios = findIosLinkRecord(store, serial);
      if (ios && setIosCallBridge(store, serial, cfg)) {
        key = `link:${ios.deviceId ?? ios.udid}`;
        this.iosRecordsCache = null;
      }
    }
    if (!key) return { ok: false, message: this.callBridgeUnknown(serial) };
    log.info(`[CallBridge] Call audio ${cfg.enabled ? 'on' : 'off'} for ${serial}`);
    const bridge = this.ensureCallBridge();
    if (cfg.enabled) await CallBridgeWindow.ensureMicAccess(true).catch(() => false);
    const status = cfg.enabled ? await bridge.verifyCable(key) : bridge.statusFor(key);
    if (status) bridge.emit('event', { type: 'status', status } satisfies CallBridgeEvent);
    return { ok: true, config: cfg, status };
  }

  /** phone:call-bridge-status — every phone that can carry a bridged call. */
  async callBridgeStatus(): Promise<CallBridgeStatus[]> {
    await this.callBridgeLanes();
    return this.callBridge?.status() ?? [];
  }

  private callBridgeUnknown(serial: string): string {
    const android = fileAndroidLinkStore(path.join(this.dir(), 'phone-link-android.json'));
    const bare = serial.startsWith('link:') ? serial.slice(5) : serial;
    if (android.all().some((r) => r.deviceId === bare)) {
      return 'Call audio needs an Android phone paired over wireless debugging (adb), or an iPhone. This phone uses the Allternit app link.';
    }
    return 'That phone is not linked to this computer.';
  }

  /** Called at launch: only brings a lane up if something was ever paired on it (zero cost for everyone else). */
  async initIfPaired(): Promise<void> {
    this.ensureIosRecovery();
    // The loopback gateway alone (no phone tools for bots): lets this computer
    // receive a phone handed over from the person's Mac before it has one.
    void this.ensureGatewayServer().catch((error) => log.warn('[Phone] gateway could not start:', error));
    const dir = path.join(app.getPath('userData'), 'phone');
    // Phone-Linked phones (iPhone helper, Android app) are reached through the
    // same gateway + MCP shim; without this, bots had no phone tools after a
    // restart until an adb phone or USB Agent woke the gateway.
    if (hasLinkedPhones(dir)) await this.startForLinkedPhones();
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
  // ── Channel readers (phone-channel-readers.ts) ─────────────────────────────
  // Read and send in messaging apps on the linked phone by driving their UI.
  // What is read stays in this computer's user data (phone-channels.json).

  private channelReader: PhoneChannelReader | null = null;

  private async channels(): Promise<PhoneChannelReader> {
    await this.ensureGatewayServer();
    const runner = this.runner;
    if (!runner) throw new Error('Phones are not set up yet.');
    this.channelReader ??= new PhoneChannelReader({
      call: (name, args) => runner.call(name, args) as never,
      cacheFile: path.join(this.dir(), 'phone-channels.json'),
      crop: (b64, r, screenWidth) => {
        const img = nativeImage.createFromBuffer(Buffer.from(b64, 'base64'));
        const size = img.getSize();
        if (!size.width) return null;
        const k = size.width / screenWidth;
        const rect = { x: Math.round(r.x * k), y: Math.round(r.y * k), width: Math.round(r.w * k), height: Math.round(r.h * k) };
        if (rect.width < 1 || rect.height < 1 || rect.x + rect.width > size.width || rect.y + rect.height > size.height) return null;
        // The bot phone shows pictures at most 280px wide: 560px covers 2x screens.
        let part = img.crop(rect);
        if (rect.width > 560) part = part.resize({ width: 560 });
        return `data:image/jpeg;base64,${part.toJPEG(65).toString('base64')}`;
      },
    });
    return this.channelReader;
  }

  /** The phone to read `app` on: a healthy Phone Link phone that has it, else an online adb phone. */
  private async channelSerial(app: string): Promise<string | null> {
    const devices = await this.linkedDevices().catch(() => [] as PhoneLinkDevice[]);
    const android = devices.filter((d) => d.platform === 'android' && d.health.state === 'HEALTHY');
    const withApp = android.find((d) => d.apps?.some((a) => a.vendor === app)) ?? android[0];
    if (withApp) return `link:${withApp.deviceId}`;
    return this.manager?.onlineSerials()[0] ?? null;
  }

  private async withChannel<T>(app: string, op: (reader: PhoneChannelReader, serial: string) => Promise<ReaderResult<T>>): Promise<ReaderResult<T>> {
    try {
      const reader = await this.channels();
      const serial = await this.channelSerial(app);
      if (!serial) return { ok: false, error: 'no_phone', message: 'Link an Android phone to read its apps here.' };
      return await op(reader, serial);
    } catch (error) {
      return { ok: false, error: 'reader_failed', message: error instanceof Error ? error.message : String(error) };
    }
  }

  async channelApps(): Promise<Array<{ app: string; name: string }>> {
    return (await this.channels().catch(() => null))?.apps() ?? [];
  }

  async channelCached(app: string, conversationId?: string) {
    return (await this.channels().catch(() => null))?.cached(app, conversationId) ?? {};
  }

  channelConversations(app: string) {
    return this.withChannel(app, (r, serial) => r.conversations(serial, app));
  }

  channelMessages(app: string, conversation: ChannelConversation) {
    return this.withChannel(app, async (r, serial) => r.messages(serial, app, conversation, await this.screenRecorder(serial)));
  }

  channelSend(app: string, conversation: ChannelConversation, text: string) {
    return this.withChannel(app, async (r, serial) => r.send(serial, app, conversation, text, await this.screenRecorder(serial)));
  }

  async channelCachedClip(app: string, conversationId: string) {
    return (await this.channels().catch(() => null))?.cachedClip(app, conversationId) ?? null;
  }

  /** Record a few seconds of a conversation on request (the ▶ fallback). */
  channelClip(app: string, conversation: ChannelConversation) {
    return this.withChannel(app, async (r, serial) => r.clip(serial, app, conversation, await this.screenRecorder(serial)));
  }

  /** Android's screenrecord over the screen lane's adb, for the phone a channel serial names; null without one. */
  private async screenRecorder(serial: string): Promise<ScreenRecorder | null> {
    const adbSerial = await this.adbSerialFor(serial);
    if (!adbSerial) return null;
    return async (size) => {
      const adb = await this.ensureAdbCall();
      const file = '/data/local/tmp/allternit-clip.mp4';
      const rec = await adb(['shell', 'screenrecord', '--time-limit', '3', '--size', `${size.width}x${size.height}`, '--bit-rate', '1500000', file], { serial: adbSerial, timeoutMs: 15_000 });
      if (rec.code !== 0) return null;
      const out = await adb(['exec-out', 'base64', file], { serial: adbSerial, timeoutMs: 15_000 });
      await adb(['shell', 'rm', '-f', file], { serial: adbSerial });
      const b64 = out.code === 0 ? out.stdout.replace(/\s+/g, '') : '';
      return b64.length > 100 ? b64 : null;
    };
  }

  /** The adb (screen lane) serial for the phone a channel serial names: itself, or the online adb phone with the same name. */
  private async adbSerialFor(serial: string): Promise<string | null> {
    const online = this.manager?.list().filter((p) => p.state === 'online') ?? [];
    if (!serial.startsWith(LINK_SERIAL_PREFIX)) return online.some((p) => p.serial === serial) ? serial : null;
    const name = (await this.linkedDevices().catch(() => [] as PhoneLinkDevice[]))
      .find((d) => LINK_SERIAL_PREFIX + d.deviceId === serial)?.name?.trim().toLowerCase();
    const match = online.find((p) => name && p.name?.trim().toLowerCase() === name) ?? (online.length === 1 ? online[0] : undefined);
    return match?.serial ?? null;
  }

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
    this.callBridge?.stop();
    this.callBridgeWindow?.destroy();
    this.callBridge = null;
    this.callBridgeWindow = null;
    this.iosRecovery?.stop();
    this.screenKeeper?.stop();
    this.screenKeeper = null;
    if (this.linkSyncTimer) clearInterval(this.linkSyncTimer);
    this.linkSyncTimer = null;
    for (const [serial, socket] of this.linkSockets) {
      socket.stop();
      this.cancelLinkDetach(serial);
      this.detachLinkConsumer(serial);
    }
    this.linkSockets.clear();
    this.streams?.stopAll();
    this.manager?.stopAll();
    this.usbAgents?.stopAll();
    this.artemis?.stopAll();
    this.gatewayClose?.();
    fs.rmSync(this.gatewayFile, { force: true });
  }
}

export const phoneService = new PhoneService();
