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
import { PhoneUserComms, type CallStateName, type CommsResult, type PhoneAppInfo, type PhoneSmsMessage, type PhoneSmsThread } from './phone-user-comms.js';
import { PhoneLinkPhoneSocket } from './phone-link-cloud.js';
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
import { fileLinkStore } from './phone-link-ios.js';
import { IosRecoveryScheduler, type IosRecoveryDeps } from './phone-ios-recovery.js';
import { createGoIosExec, createGoIosSpawn, ensureDdi, ensureGoIos, GoIos } from './phone-goios.js';
import { ensureRunnerArtifacts, pickSigner } from './phone-ios-runner.js';
import { readPairRecord, startLoopbackForward } from './phone-usbmux.js';
import { hasLinkedPhones } from './phone-link-stores.js';
import { PhoneLinkBackend, cachedHostResolver, linkedPhoneSource, probeLinkedPhone, rememberLinkedApps,
  rememberLinkedHost, type LinkedPhone } from './phone-link-backend.js';
import { fileAndroidLinkStore } from './phone-link-android.js';
import { nodeDiscoveryDeps } from './phone-link-discovery.js';
import { PHONE_LINK_MESH_ENABLED } from './phone-link-mesh.js';
import { meshManager } from './mesh-manager.js';
import { PhoneLinkHealthMonitor } from './phone-link-health.js';
import type { PhoneLinkDevice, PhoneLinkHandoverResult, PhoneLinkHandoverTarget } from './phone-link-types.js';
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
          ? {
              meshProxy: async (meshHost: string) => {
                const url = await meshManager.proxyFor(`http://${meshHost}`);
                // The lane speaks to a loopback host, not a URL.
                return url.replace(/^https?:\/\//, '').replace(/\/$/, '');
              },
            }
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

  /** Linked phones with health, for phone-link:devices (Settings → Subscriptions › Linked phones). */
  linkedDevices(): Promise<PhoneLinkDevice[]> {
    return this.ensureLinkedBackend().linkedDevices();
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
          const online = new Set(manager.onlineSerials());
          for (const serial of this.streams?.activeSerials() ?? []) {
            if (!online.has(serial)) this.streams?.deviceDisconnected(serial);
          }
          this.syncLinkSockets();
        });
        this.manager = manager;
        this.streams = new PhoneStreamManager({
          createSession: (serial, opts) =>
            new ScrcpySession(
              {
                adb: nodeScrcpyAdb(this.adbPath ?? '', serial),
                serial,
                serverPath: this.ensureScrcpyServerPath(),
                connect: nodeScrcpyConnect,
                log: (message) => log.info('[PhoneStream]', message),
              },
              opts,
            ),
        });
        this.linkSyncTimer = setInterval(() => this.syncLinkSockets(), LINK_SYNC_INTERVAL_MS);
        this.linkSyncTimer.unref?.();
        this.syncLinkSockets();
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

  /** The in-app live view manager (scrcpy-server video + input); created with the Android lane. */
  async ensureStreams(): Promise<PhoneStreamManager> {
    await this.ensureReady();
    return this.streams!;
  }

  /**
   * Start a live stream on request only. The phone must be online; the
   * pinned scrcpy-server is downloaded on first use, not here.
   */
  async startStream(serial: string, opts?: PhoneStreamOptions): Promise<{ ok: true } | { ok: false; message: string }> {
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

  /** The PhoneViewport apps sheet inventory (per serial, unlike the comms lane). */
  async listApps(serial: string): Promise<CommsResult<{ apps: PhoneAppInfo[] }>> {
    const manager = await this.ensureReady();
    const phone = manager.list().find((p) => p.serial === serial);
    if (!phone || phone.state !== 'online') return { ok: false as const, message: 'That phone is not connected.' };
    return (await this.ensureComms()).listApps(serial);
  }

  getComms(): PhoneUserComms | null {
    return this.comms;
  }

  private async commsWith<T>(fn: (serial: string, comms: PhoneUserComms) => Promise<CommsResult<T>>): Promise<CommsResult<T>> {
    const target = await this.commsTarget();
    if ('ok' in target) return target;
    return fn(target.serial, await this.ensureComms());
  }

  getStreamManager(): PhoneStreamManager | null {
    return this.streams;
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
  private syncLinkSockets(): void {
    if (!authManager.getSessionSnapshot()) return;
    const phones = this.manager?.list() ?? [];
    const seen = new Set<string>();
    for (const phone of phones) {
      if (!phone.serial) continue;
      seen.add(phone.serial);
      if (!this.linkSockets.has(phone.serial)) {
        this.linkSockets.set(phone.serial, this.createLinkSocket(phone.serial));
      }
      this.linkSockets.get(phone.serial)?.notePhoneChanged();
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
      // Both relay legs must terminate at the same api the web app talks to
      // (the cloud transport uses the same base). In dev, point Desktop at
      // the local api with ALLTERNIT_CLOUD_API_URL=http://127.0.0.1:18013.
      apiUrl: () => (process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API).replace(/\/$/, ''),
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
        const p = this.manager?.list().find((entry) => entry.serial === serial);
        return { serial, model: p?.model ?? '', name: p?.name || p?.model || 'Linked phone' };
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
      const streams = await this.ensureStreams();
      const consumer: StreamConsumer = {
        onMeta: (_s, meta) => socket.push({ kind: 'meta', meta }),
        onChunk: (_s, chunk) => socket.push({ kind: 'chunk', chunk }),
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
    this.iosRecovery?.stop();
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
