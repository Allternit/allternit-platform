/**
 * Phone Link, Android: the cable setup that installs the Phone Link app
 * (com.allternit.phonelink) and leaves it reachable over Wi-Fi.
 *
 *   usb_debugging → installing → pairing → ready_wifi
 *
 * The old flow moved the phone to `adb tcpip` + `adb connect`, which does not
 * survive a phone reboot (measured). Instead Desktop now installs the Phone
 * Link Android app (source in surfaces/allternit-mobile/android, published as
 * a pinned APK by phone-android-helper.ts). The app runs the same iCTRL
 * protocol v1 server on port 8200 as the iPhone runner, starts itself on
 * boot, and was proven live 2026-10-17 on Android 14: paired over the cable
 * with no tap on the phone, read ChatGPT over Wi-Fi, back 35 s after a
 * reboot with no adb.
 *
 * Everything the phone reports is polled; nothing needs the user's "I've
 * done it". Re-running setup on a linked phone works: the APK is
 * reinstalled/upgraded, provisioning re-arms the one-shot pairing grant, and
 * pairing replaces the stored key.
 */

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import type { AdbCall } from './phone-tools.js';
import type { DriverContext, PhoneLinkDriver } from './phone-link.js';
import type { HandedOverFrom, PhoneLinkApp, PhoneLinkStatus } from './phone-link-types.js';
import { ICTRL_PORT, ictrlStoreKey, type IctrlLike, type IctrlWhoami } from './ictrl-client.js';
import { AndroidHelperUnavailableError, type AndroidHelper } from './phone-android-helper.js';
import { findPhoneHost, nodeDiscoveryDeps, type DiscoveryDeps } from './phone-link-discovery.js';
import { ANDROID_APP_PACKAGES, catalogPhoneApps } from './phone-app-catalog.js';
import {
  MESH_PENDING_DETAIL,
  PHONE_LINK_MESH_ENABLED,
  pollMeshHost,
  shellQuote,
  type PhoneLinkMeshDep,
} from './phone-link-mesh.js';

/** Catalog id → Android packages (phone-app-catalog.json). */
export const ANDROID_VENDOR_PACKAGES = ANDROID_APP_PACKAGES;

export const PHONE_LINK_PACKAGE = 'com.allternit.phonelink';
const ACCESSIBILITY_SERVICE = `${PHONE_LINK_PACKAGE}/.PhoneLinkAccessibilityService`;
/** How `dumpsys accessibility` spells the service once enabled (full class name, not the short form). */
const ACCESSIBILITY_SERVICE_FULL = `${PHONE_LINK_PACKAGE}/${PHONE_LINK_PACKAGE}.PhoneLinkAccessibilityService`;

/**
 * Enabled is not enough: the system must also have bound the service, which
 * `dumpsys accessibility` lists by label under "Bound services".
 */
export function accessibilityBound(dumpsys: string): boolean {
  return dumpsys.includes(ACCESSIBILITY_SERVICE_FULL) && /Bound services:\{[^\n]*label=Allternit Phone Link/.test(dumpsys);
}
const PROVISION_ACTION = `${PHONE_LINK_PACKAGE}.PROVISION`;
/** Where the app's shell server runs scrcpy-server from (ShellServerProtocol.SCRCPY_JAR). */
export const HELPER_SCRCPY_JAR = '/data/local/tmp/allternit-scrcpy-server.jar';
const POLL_MS = 1500;
const BIND_TIMEOUT_MS = 10_000;

export interface AdbDeviceLine {
  serial: string;
  state: string;
  model?: string;
}

export function parseAdbDevices(stdout: string): AdbDeviceLine[] {
  return stdout
    .split('\n')
    .slice(1)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [serial, state, ...rest] = line.split(/\s+/);
      const model = rest.find((part) => part.startsWith('model:'))?.slice('model:'.length).replace(/_+/g, ' ').trim();
      return { serial, state, model };
    });
}

/** versionCode of an installed package from `dumpsys package`, null when absent. */
export function parsePackageVersionCode(stdout: string): number | null {
  const match = /(?:^|\s)versionCode=(\d+)/.exec(stdout);
  return match ? Number(match[1]) : null;
}

export function androidVendorApps(packages: Iterable<string>): PhoneLinkApp[] {
  return catalogPhoneApps(packages, 'android');
}

export function installedVendorApps(packagesStdout: string): PhoneLinkApp[] {
  return androidVendorApps(packagesStdout.split('\n').map((line) => line.trim().replace(/^package:/, '')).filter(Boolean));
}

/** First IPv4 on wlan0 from `ip -f inet addr show wlan0`. */
export function parseWlanIp(stdout: string): string | null {
  return /inet (\d+\.\d+\.\d+\.\d+)\//.exec(stdout)?.[1] ?? null;
}

/** `device_id` from the PROVISION broadcast's `data="provisioned device_id=…"`. */
export function parseProvisionDeviceId(stdout: string): string | null {
  return /device_id=([\w.-]+)/.exec(stdout)?.[1] ?? null;
}

/** Non-secret per-phone state, kept so a later start can reconnect instead of re-installing. */
export interface AndroidLinkRecord {
  deviceId: string;
  serial: string;
  deviceName?: string;
  osVersion?: string;
  lanHost?: string;
  /** `<100.x>:8200` on the user's mesh, saved once the phone reports the join running. */
  meshHost?: string;
  /** The mesh join didn't finish during setup; a later setup retries it. Never a failure. */
  meshPending?: boolean;
  /** Vendor apps found last time. */
  apps?: PhoneLinkApp[];
  pairedAt: string;
  helperVersion?: string;  /** Received from another computer by pairing handover (phone-link-handover.ts). */
  handedOverFrom?: HandedOverFrom;
}

export interface AndroidLinkStore {
  load(deviceId: string): AndroidLinkRecord | null;
  all(): AndroidLinkRecord[];
  save(record: AndroidLinkRecord): void;
}

export function fileAndroidLinkStore(file: string): AndroidLinkStore {
  const read = (): Record<string, AndroidLinkRecord> => {
    try {
      return JSON.parse(fs.readFileSync(file, 'utf8'));
    } catch {
      return {};
    }
  };
  return {
    load: (deviceId) => read()[deviceId] ?? null,
    all: () => Object.values(read()),
    save: (record) => {
      const all = read();
      all[record.deviceId] = record;
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, JSON.stringify(all, null, 2), { mode: 0o600 });
    },
  };
}

export interface AndroidDriverDeps {
  /** Lazily resolved so adb is only downloaded when Android setup starts. */
  adb: () => Promise<AdbCall>;
  /** Pinned-download APK (phone-android-helper.ts); throws AndroidHelperUnavailableError until pinned. */
  helper: (dataDir: string) => Promise<AndroidHelper>;
  /** Holds the helper cache and the link store (userData/phone). */
  dataDir: string;
  /** Client for this phone's key (secret store entry per device id) at `host`. */
  ictrl: (storeKey: string, host: string) => IctrlLike;
  /** Subnet scan used to find the phone after its address changed. */
  discovery?: DiscoveryDeps;
  /**
   * Mesh credentials source for the away-from-home path. Only used when
   * PHONE_LINK_MESH_ENABLED is on; every failure is non-fatal. The auth key
   * it returns is never logged or stored.
   */
  mesh?: PhoneLinkMeshDep;
  store: AndroidLinkStore;
  /**
   * The pinned scrcpy-server jar (phone-scrcpy.ts ensureScrcpyServer). Setup
   * leaves a copy where the app's screen service runs it from, so the live
   * screen works through Phone Link without adb. Optional: without it setup
   * skips that copy.
   */
  scrcpyServer?: (dataDir: string) => Promise<string>;
  clientName?: string;
  now?: () => number;
  logger?: (message: string) => void;
}

type Base = Omit<PhoneLinkStatus, 'platform' | 'state' | 'instruction'>;

/**
 * Run setup on one phone adb already reaches over TCP instead of waiting for
 * a USB cable: a phone added by address (PhoneDeviceManager.addByAddress),
 * such as an Allternit cloud phone. `linkHost` is where the Phone Link app's
 * port is published (a cloud phone has no Wi-Fi; its host publishes 8200 on
 * the mesh); without it setup reads the phone's wlan0 address as usual.
 */
export interface AndroidSetupTarget {
  serial: string;
  /** `host:port` of the Phone Link app; a mesh address (100.64.0.0/10) is reached through `deps.mesh.loopbackFor`. */
  linkHost?: string;
}

function isMeshAddress(hostPort: string): boolean {
  const match = /^100\.(\d{1,3})\.\d{1,3}\.\d{1,3}:\d{1,5}$/.exec(hostPort);
  return !!match && Number(match[1]) >= 64 && Number(match[1]) <= 127;
}

class SetupStop extends Error {}

export class AndroidDriver implements PhoneLinkDriver {
  private adb: AdbCall | null = null;
  private forwardPort: string | null = null;
  private base: Base = {};

  constructor(
    private readonly deps: AndroidDriverDeps,
    private readonly target?: AndroidSetupTarget,
  ) {}

  dispose(): void {
    void this.removeForward();
  }

  private log(message: string): void {
    this.deps.logger?.(`[phone-link android] ${message}`);
  }

  private set(ctx: DriverContext, state: PhoneLinkStatus['state'], instruction: string, extra: Partial<Base> = {}): void {
    ctx.set({ ...this.base, detail: undefined, progress: undefined, ...extra, state, instruction });
  }

  private fail(ctx: DriverContext, code: string, instruction: string, message: string, retryable: boolean): never {
    ctx.set({ ...this.base, state: 'error', instruction, error: { code, message, retryable } });
    throw new SetupStop();
  }

  private check(ctx: DriverContext): void {
    if (ctx.isCancelled()) throw new SetupStop();
  }

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  async run(ctx: DriverContext): Promise<void> {
    try {
      await this.walk(ctx);
    } catch (error) {
      if (error instanceof SetupStop || ctx.isCancelled()) return;
      throw error;
    }
  }

  private async removeForward(): Promise<void> {
    const port = this.forwardPort;
    this.forwardPort = null;
    if (port && this.adb) await this.adb(['forward', '--remove', `tcp:${port}`]).catch(() => {});
  }

  private async walk(ctx: DriverContext): Promise<void> {
    const adb = await this.deps.adb();
    this.adb = adb;

    // 1. A USB-connected phone that allowed this computer (or the
    // address-added phone this run targets).
    let usb: AdbDeviceLine | undefined;
    const target = this.target;
    for (let tries = 0; target; tries += 1) {
      this.check(ctx);
      const row = parseAdbDevices((await adb(['devices', '-l'])).stdout).find((d) => d.serial === target.serial);
      if (row?.state === 'device') {
        usb = row;
        break;
      }
      if (tries === 0) await adb(['connect', target.serial], { timeoutMs: 15_000 }).catch(() => null);
      if (tries >= 20) {
        this.fail(ctx, 'phone_unreachable', `Couldn't reach the phone at ${target.serial}. Check it is running, then try again.`, `adb shows ${row?.state ?? 'no device'} for ${target.serial}`, true);
      }
      this.set(ctx, 'usb_debugging', 'Connecting to your phone…', { detail: target.serial });
      await ctx.sleep(POLL_MS);
    }
    while (!usb) {
      this.check(ctx);
      const devices = parseAdbDevices((await adb(['devices', '-l'])).stdout).filter((d) => !d.serial.includes(':'));
      usb = devices.find((d) => d.state === 'device');
      if (usb) break;
      const unauthorized = devices.some((d) => d.state === 'unauthorized');
      this.set(
        ctx,
        'usb_debugging',
        unauthorized ? 'On your phone, tap Allow on "Allow USB debugging?".' : 'Plug your phone in with a cable and turn on USB debugging in Developer options.',
        {
          detail: unauthorized ? undefined : 'Settings › About phone › tap Build number 7 times, then Settings › Developer options › USB debugging.',
        },
      );
      await ctx.sleep(POLL_MS);
    }

    const serial = usb.serial;
    const prop = async (name: string) => (await adb(['shell', 'getprop', name], { serial })).stdout.trim();
    const deviceName = usb.model ?? (await prop('ro.product.model'));
    const osVersion = `Android ${await prop('ro.build.version.release')}`;
    this.base = { deviceName, osVersion };

    // 2. The helper APK: skip when the phone already carries this version or newer, else install.
    this.set(ctx, 'installing', 'Installing the Allternit helper on your phone…', { progress: 0.15 });
    let helper: AndroidHelper;
    try {
      helper = await this.deps.helper(this.deps.dataDir);
    } catch (error) {
      this.check(ctx);
      this.fail(
        ctx,
        'helper_unavailable',
        "This version of Allternit can't set up Android phones yet. Update Allternit, then try again.",
        error instanceof Error ? error.message : String(error),
        false,
      );
    }
    const installedCode = parsePackageVersionCode((await adb(['shell', 'dumpsys', 'package', PHONE_LINK_PACKAGE], { serial })).stdout);
    if (!(helper.manifest && installedCode !== null && installedCode >= helper.manifest.versionCode)) {
      const install = await adb(['install', '-r', '-g', helper.apk], { serial });
      this.check(ctx);
      if (install.code !== 0 || /Failure/i.test(install.stdout + install.stderr)) {
        this.fail(ctx, 'install_failed', "Couldn't install Allternit Phone Link on your phone. Unlock it and try again.", (install.stdout + install.stderr).trim(), true);
      }
    }

    // 3. Provision the app exactly like its own provisioning script does.
    this.set(ctx, 'installing', 'Turning on the Allternit helper on your phone…', { progress: 0.4 });
    await adb(['shell', 'appops', 'set', PHONE_LINK_PACKAGE, 'ACCESS_RESTRICTED_SETTINGS', 'allow'], { serial }).catch(() => {});
    const current = (await adb(['shell', 'settings', 'get', 'secure', 'enabled_accessibility_services'], { serial })).stdout.trim();
    const services = current === 'null' || current === '' ? [] : current.split(':').filter(Boolean);
    if (!services.includes(ACCESSIBILITY_SERVICE)) services.push(ACCESSIBILITY_SERVICE);
    this.check(ctx);
    await adb(['shell', 'settings', 'put', 'secure', 'enabled_accessibility_services', services.join(':')], { serial });
    await adb(['shell', 'settings', 'put', 'secure', 'accessibility_enabled', '1'], { serial });
    await adb(['shell', 'cmd', 'deviceidle', 'whitelist', `+${PHONE_LINK_PACKAGE}`], { serial });
    await adb(['shell', 'pm', 'grant', PHONE_LINK_PACKAGE, 'android.permission.POST_NOTIFICATIONS'], { serial }).catch(() => {});
    await adb(['shell', 'am', 'start-foreground-service', '-n', `${PHONE_LINK_PACKAGE}/.PhoneLinkService`], { serial });
    const quotedName = `'${deviceName.replace(/'/g, '')}'`;
    const provision = await adb(
      [
        'shell', 'am', 'broadcast', '-a', PROVISION_ACTION, '-n', `${PHONE_LINK_PACKAGE}/.ProvisionReceiver`,
        '--ez', 'arm_pairing', 'true', '--ez', 'allow_remote_access', 'true', '--ei', 'listen_port', String(ICTRL_PORT),
        '--es', 'device_name', quotedName,
      ],
      { serial },
    );
    this.check(ctx);
    const deviceId = parseProvisionDeviceId(provision.stdout + provision.stderr);
    if (!deviceId) {
      this.fail(ctx, 'provision_failed', "Couldn't set up the helper on your phone. Try again.", (provision.stdout + provision.stderr).trim() || 'the PROVISION broadcast gave no device id', true);
    }
    await this.prepareScreenService(adb, serial);
    let bound = false;
    for (let waited = 0; waited < BIND_TIMEOUT_MS; waited += 500) {
      this.check(ctx);
      const out = await adb(['shell', 'dumpsys', 'accessibility'], { serial });
      if (accessibilityBound(out.stdout)) {
        bound = true;
        break;
      }
      await ctx.sleep(500);
    }
    if (!bound) {
      this.fail(ctx, 'accessibility_blocked', 'On your phone, open Settings › Accessibility › Allternit Phone Link and turn it on, then try again.', 'the accessibility service never bound', true);
    }

    // 4. Pair over the cable: the armed one-shot grant approves with no tap on the phone.
    this.set(ctx, 'pairing', 'Connecting to your phone…', { progress: 0.7 });
    try {
      const fwd = await adb(['forward', 'tcp:0', `tcp:${ICTRL_PORT}`], { serial });
      this.forwardPort = fwd.stdout.trim();
      this.check(ctx);
      const storeKey = ictrlStoreKey(`android:${deviceId}`);
      const cable = this.deps.ictrl(storeKey, `127.0.0.1:${this.forwardPort}`);
      let who: IctrlWhoami | null = null;
      for (let i = 0; i < 30 && !who; i++) {
        this.check(ctx);
        who = await cable.whoami().catch(() => null);
        if (!who) await ctx.sleep(1000);
      }
      if (!who || who.device_id !== deviceId) {
        this.fail(ctx, 'pair_failed', "Couldn't connect to the helper on your phone. Try again.", who ? `expected device_id ${deviceId}, got ${who.device_id}` : 'the helper did not answer the cable forward', true);
      }
      try {
        await cable.pair(this.deps.clientName ?? `Allternit Desktop (${os.hostname()})`, { deviceId, deviceName });
      } catch (error) {
        this.fail(ctx, 'pair_failed', "Couldn't connect to the helper on your phone. Try again.", error instanceof Error ? error.message : String(error), true);
      }

      // 5. Prove the app answers over Wi-Fi before the cable can come out.
      this.set(ctx, 'pairing', 'Moving the connection to Wi-Fi…', { progress: 0.85 });
      // A mesh link address is dialled through a loopback proxy this session;
      // the record keeps the mesh address as meshHost so later sessions
      // reconnect through the mesh again (phone-link-backend meshProxy).
      const meshLinkHost = target?.linkHost && isMeshAddress(target.linkHost) ? target.linkHost : undefined;
      let linkHost = target?.linkHost;
      if (meshLinkHost) {
        linkHost = await this.deps.mesh?.loopbackFor?.(meshLinkHost).catch(() => undefined);
        if (!linkHost) {
          this.fail(ctx, 'mesh_unavailable', 'Sign in to Allternit on this computer to reach your cloud phone, then try again.', `no mesh proxy for ${meshLinkHost}`, true);
        }
      }
      const ip = linkHost ? null : parseWlanIp((await adb(['shell', 'ip', '-f', 'inet', 'addr', 'show', 'wlan0'], { serial })).stdout);
      if (!linkHost && !ip) {
        this.fail(ctx, 'no_wifi', 'Turn on Wi-Fi on your phone, then try again.', 'The phone has no Wi-Fi address.', true);
      }
      const lanHost = linkHost ?? `${ip}:${ICTRL_PORT}`;
      const lan = cable.withHost(lanHost);
      let reachable = false;
      for (let i = 0; i < 10 && !reachable; i++) {
        this.check(ctx);
        const info = await lan.call<{ device_id?: string }>('device_info', {}, 5000).catch(() => null);
        reachable = info?.device_id === deviceId;
        if (!reachable) await ctx.sleep(1000);
      }
      if (!reachable) {
        this.fail(
          ctx,
          'wifi_unreachable',
          linkHost ? "Couldn't reach Phone Link on that phone over the network. Try again." : 'Your phone and this computer need to be on the same Wi-Fi. Connect both, then try again.',
          `no answer from ${lanHost} with the paired key`,
          true,
        );
      }

      // 5½. Away-from-home (development switch): hand the app its mesh
      // credentials and wait for the join. Never fails setup — a phone that
      // missed the mesh is still a working LAN phone.
      // A phone with a published link address is already on the mesh (cloud
      // phones live on a mesh host), so it skips the join.
      const mesh: { meshHost?: string; meshPending?: boolean } = linkHost ? (meshLinkHost ? { meshHost: meshLinkHost } : {}) : await this.tryMeshJoin(ctx, adb, serial, lanHost);

      // 6. Which AI apps are on the phone.
      const listed = await lan.call<{ apps?: { package?: string; bundle_id?: string }[] }>('list_apps', {}, 10_000).catch(() => null);
      const apps = androidVendorApps((listed?.apps ?? []).map((app) => app.package ?? app.bundle_id ?? '').filter(Boolean));

      // 7. Remember the phone, then the cable can come out.
      this.deps.store.save({
        deviceId,
        serial,
        deviceName,
        osVersion,
        lanHost,
        ...(mesh.meshHost || mesh.meshPending ? mesh : {}),
        apps,
        pairedAt: new Date(this.now()).toISOString(),
        helperVersion: helper.manifest?.version,
      });
      this.set(ctx, 'ready_wifi', target ? 'Connected.' : 'Connected. You can unplug the cable.', {
        detail: mesh.meshPending ? MESH_PENDING_DETAIL : (meshLinkHost ?? lanHost),
        apps,
        progress: undefined,
      });
    } finally {
      await this.removeForward();
    }
  }

  /**
   * The app's screen service (Phone Link 0.3+, Android 13+) starts
   * scrcpy-server itself, so the live screen keeps working off the cable and
   * away from home: it needs WRITE_SECURE_SETTINGS (to turn wireless debugging
   * on for its own launch) and the pinned scrcpy-server jar on the phone.
   * Best-effort: a phone without them still links; its live screen then needs
   * Wireless debugging.
   */
  private async prepareScreenService(adb: AdbCall, serial: string): Promise<void> {
    const grant = await adb(['shell', 'pm', 'grant', PHONE_LINK_PACKAGE, 'android.permission.WRITE_SECURE_SETTINGS'], { serial }).catch(() => null);
    if (!grant || grant.code !== 0) this.log(`screen service: WRITE_SECURE_SETTINGS not granted (${(grant?.stderr || grant?.stdout || 'adb failed').trim().slice(0, 160)})`);
    if (!this.deps.scrcpyServer) return;
    try {
      const jar = await this.deps.scrcpyServer(this.deps.dataDir);
      const push = await adb(['push', jar, HELPER_SCRCPY_JAR], { serial });
      if (push.code !== 0) this.log(`screen service: scrcpy-server push failed (${(push.stderr || push.stdout).trim().slice(0, 160)})`);
    } catch (error) {
      this.log(`screen service: scrcpy-server unavailable (${error instanceof Error ? error.message : String(error)})`);
    }
  }

  /**
   * Re-provision the app with the two mesh extras and poll its LAN /whoami
   * for `mesh_state: 'running'` + a mesh_host (60 s budget; the live join
   * took < 10 s). Resolves `{ meshHost }` on success and `{ meshPending: true }`
   * on any failure — not signed in, enroll error, broadcast error, timeout —
   * so setup always finishes ready_wifi. The auth key goes to the phone
   * shell and nowhere else: never a log line, never a status field.
   */
  private async tryMeshJoin(
    ctx: DriverContext,
    adb: AdbCall,
    serial: string,
    lanHost: string,
  ): Promise<{ meshHost?: string; meshPending?: boolean }> {
    if (!PHONE_LINK_MESH_ENABLED || !this.deps.mesh) return {};
    let enrollment: { controlUrl: string; authKey: string };
    try {
      enrollment = await this.deps.mesh.enroll();
    } catch {
      this.log('mesh enrollment unavailable; the phone stays LAN-only for now');
      return { meshPending: true };
    }
    try {
      // arm_pairing stays off: the phone is already paired, and a stray
      // armed grant on a mesh-reachable phone is exactly what the app's
      // viaMesh rules refuse. Re-provisioning keeps the stored mesh settings.
      await adb(
        [
          'shell', 'am', 'broadcast', '-a', PROVISION_ACTION, '-n', `${PHONE_LINK_PACKAGE}/.ProvisionReceiver`,
          '--ez', 'arm_pairing', 'false', '--ez', 'allow_remote_access', 'true', '--ei', 'listen_port', String(ICTRL_PORT),
          '--es', 'mesh_control_url', shellQuote(enrollment.controlUrl),
          '--es', 'mesh_auth_key', shellQuote(enrollment.authKey),
        ],
        { serial },
      );
      this.check(ctx);
      const fetchWhoami = (this.deps.discovery ?? nodeDiscoveryDeps).fetchWhoami;
      const meshHost = await pollMeshHost(fetchWhoami, lanHost, {
        check: () => this.check(ctx),
        sleep: (ms) => ctx.sleep(ms),
        now: () => this.now(),
      });
      if (!meshHost) {
        this.log('mesh join did not report running in time; setup continues without it');
        return { meshPending: true };
      }
      this.log(`mesh join running at ${meshHost}`);
      return { meshHost };
    } catch (error) {
      if (error instanceof SetupStop) throw error;
      this.log('mesh setup failed; the phone stays LAN-only for now');
      return { meshPending: true };
    }
  }
}

/**
 * Bring a linked phone back after its address changed or it rebooted (the app
 * starts itself on boot, so there is no cable fallback to drive). The saved
 * LAN host is tried first, then the subnet is scanned for this device id.
 * Returns the LAN host once a sealed call answers with the stored key.
 */
export async function reconnectAndroid(deps: AndroidDriverDeps, deviceId: string): Promise<string | null> {
  const record = deps.store.load(deviceId);
  if (!record) return null;
  const storeKey = ictrlStoreKey(`android:${deviceId}`);
  const probe = async (host?: string) => (host ? deps.ictrl(storeKey, host).call('device_info', {}, 5000).then(() => host, () => null) : null);
  const direct = await probe(record.lanHost);
  if (direct) return direct;
  try {
    const found = await findPhoneHost(deviceId, record.lanHost, deps.discovery ?? nodeDiscoveryDeps);
    if (found && (await probe(found))) {
      deps.store.save({ ...record, lanHost: found });
      return found;
    }
  } catch (error) {
    deps.logger?.(`[phone-link android] reconnect failed: ${error instanceof Error ? error.message : String(error)}`);
  }
  return null;
}
