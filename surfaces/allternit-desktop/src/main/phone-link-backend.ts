/**
 * Phone Link lane for the bot phone tools (phone-tools.ts). A Phone-Linked
 * phone — iPhone with our iCTRL helper, Android with the Phone Link app — is
 * invisible to adb and the USB Agent, but both helpers speak iCTRL protocol
 * v1 (ictrl-client.ts), so one lane serves both. Serials are "link:<deviceId>".
 *
 * The backend implements the same UsbPhoneBackend interface the USB Agent
 * lane does, with two additions: linkedDevices() (phone.devices with health)
 * and nothing else — PhoneToolRunner consults it exactly like usb.
 *
 * Coordinates: taps and swipes arrive in the latest screenshot's pixel space,
 * like the other lanes. The helpers speak in their native space (points on
 * iOS, pixels on Android), so when a screenshot shows the image is scaled
 * relative to the phone's space, the factor is learned from that screenshot
 * and applied to taps (iPhone: a 3× JPEG of a 393-pt-wide screen divides by 3).
 */

import { IctrlError, ictrlStoreKey } from './ictrl-client.js';
import { IOS_VENDOR_BUNDLES, type IctrlLike, type IosLinkStore } from './phone-link-ios.js';
import { ANDROID_VENDOR_PACKAGES, type AndroidLinkStore } from './phone-link-android.js';
import { catalogApp, catalogAppByName, catalogPhoneApps, nameCatalogApps } from './phone-app-catalog.js';
import { scanLan, type DiscoveryDeps } from './phone-link-discovery.js';
import { PACKAGE_RE, resolveApp, type ToolErrorCode, type ToolResult, type UsbPhoneBackend } from './phone-tools.js';
import {
  PhoneLinkHealthMonitor,
  type HealthEvidence,
  type LinkedPhone,
  type LinkedPhoneSource,
} from './phone-link-health.js';
import type { PhoneLinkApp, PhoneLinkDevice, PhoneLinkHealth } from './phone-link-types.js';

export type { LinkedPhone, LinkedPhoneSource };

/** One linked phone as phone.devices lists it: the runner's shape (serial, via) plus the UI's (deviceId, apps). */
export interface LinkedPhoneDeviceInfo {
  serial: string;
  platform: 'ios' | 'android';
  via: 'phone-link';
  route: 'iphone-app-ictrl' | 'android-app';
  name?: string;
  health: PhoneLinkHealth;
  deviceId: string;
  apps?: PhoneLinkApp[];
  /** Which transport last reached the phone ('lan' or 'mesh'), when known. */
  path?: 'lan' | 'mesh';
}

export const LINK_SERIAL_PREFIX = 'link:';

const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

const COORD_MAX = 20_000;
const isInt = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0 && (v as number) <= COORD_MAX;

/**
 * Where a linked phone answers now. `lastPath` reports which transport the
 * most recent resolution used ('lan' or 'mesh'); resolvers that never check
 * the mesh simply don't carry it.
 */
export type HostResolver = ((phone: LinkedPhone) => Promise<string | null>) & {
  lastPath?: (deviceId: string) => 'lan' | 'mesh' | undefined;
};

export interface PhoneLinkBackendDeps {
  phones: LinkedPhoneSource;
  /** Client for this phone's key (secret store entry per phone) at `host`. */
  client: (phone: LinkedPhone, host: string) => IctrlLike;
  /** Where the helper answers; default is the phone's saved lanHost. The LAN-scanning findPhoneHost lands on a sibling branch — this is its seam. */
  resolveHost?: HostResolver;
  health?: PhoneLinkHealthMonitor;
  /** Save a phone's re-read app list (phone-app-catalog.json matches). */
  saveApps?: (phone: LinkedPhone, apps: PhoneLinkApp[]) => void;
  now?: () => number;
  logger?: (message: string) => void;
}

/** How often a healthy phone's apps are re-read, so an app installed after setup shows up. */
export const APPS_REFRESH_MS = 10 * 60_000;

export class PhoneLinkBackend implements UsbPhoneBackend {
  /** image pixels per phone point, learned from a screenshot (deviceId → scale). */
  /** Latest screenshot's pixel size per phone (for direction swipes). */
  private readonly screens = new Map<string, { width: number; height: number }>();
  private readonly appsReadAt = new Map<string, number>();

  constructor(private readonly deps: PhoneLinkBackendDeps) {}

  /**
   * Re-read which catalog apps a healthy phone has, at most every
   * APPS_REFRESH_MS, without holding up phone.devices. Android's helper lists
   * every package, so its answer replaces the saved list. The iPhone runner's
   * list can be partial (iOS 26 lists nothing over Wi-Fi; only apps it has
   * seen open), so on iPhone new apps are added and none are removed — the
   * full list comes from the cable at setup.
   */
  private refreshApps(phones: LinkedPhone[]): void {
    const save = this.deps.saveApps;
    if (!save) return;
    for (const phone of phones) {
      if (this.deps.health?.health(phone.deviceId)?.state !== 'HEALTHY') continue;
      const last = this.appsReadAt.get(phone.deviceId);
      if (last !== undefined && this.now() - last < APPS_REFRESH_MS) continue;
      this.appsReadAt.set(phone.deviceId, this.now());
      void this.invoke<{ apps?: { package?: string; bundle_id?: string }[] }>(phone, 'list_apps', {}, 10_000).then((res) => {
        if (!res.ok) return;
        const ids = (res.value?.apps ?? []).map((app) => app.package ?? app.bundle_id ?? '').filter(Boolean);
        if (ids.length === 0) return;
        const found = catalogPhoneApps(ids, phone.platform);
        const next =
          phone.platform === 'android'
            ? found
            : [...(phone.apps ?? []), ...found.filter((app) => !(phone.apps ?? []).some((had) => had.vendor === app.vendor))];
        const same = next.length === (phone.apps ?? []).length && next.every((app, i) => app.vendor === phone.apps?.[i]?.vendor);
        if (!same) save(phone, next);
      });
    }
  }

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  has(serial: string): boolean {
    return this.serials().includes(serial);
  }

  /** Online = health is HEALTHY or DEVICE_LOCKED (or not checked yet). */
  serials(): string[] {
    return this.deps
      .phones()
      .filter((p) => {
        const health = this.deps.health?.fresh(p.deviceId) ?? null;
        return !health || health.state === 'HEALTHY' || health.state === 'DEVICE_LOCKED';
      })
      .map((p) => LINK_SERIAL_PREFIX + p.deviceId);
  }

  /** phone.devices: linked phones with their health, refreshing stale entries. */
  async linkedDevices(): Promise<LinkedPhoneDeviceInfo[]> {
    const phones = this.deps.phones();
    if (this.deps.health) await this.deps.health.refreshStale(phones.map((p) => p.deviceId));
    this.refreshApps(phones);
    const pathOf = this.deps.resolveHost?.lastPath;
    return phones.map((p) => ({
      serial: LINK_SERIAL_PREFIX + p.deviceId,
      deviceId: p.deviceId,
      platform: p.platform,
      via: 'phone-link' as const,
      route: p.platform === 'ios' ? ('iphone-app-ictrl' as const) : ('android-app' as const),
      name: p.name,
      health: this.deps.health?.health(p.deviceId) ?? {
        state: 'UNKNOWN',
        checkedAt: new Date(this.now()).toISOString(),
        recovery: 'automatic',
        detail: 'Health has not been checked yet.',
      },
      apps: p.apps,
      ...(pathOf?.(p.deviceId) ? { path: pathOf(p.deviceId)! } : {}),
    }));
  }

  async call(name: string, serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const phone = this.deps.phones().find((p) => serial === LINK_SERIAL_PREFIX + p.deviceId);
    if (!phone) return fail('device_offline', `Phone ${serial} is not connected.`);
    switch (name) {
      case 'phone.screenshot':
        return await this.screenshot(phone);
      case 'phone.tap':
        return await this.tap(phone, a);
      case 'phone.swipe':
        return await this.swipe(phone, a);
      case 'phone.type':
        return await this.typeText(phone, a);
      case 'phone.home':
        return await this.simple(phone, 'home');
      case 'phone.back':
        if (phone.platform === 'ios') {
          return fail('unsupported', 'iPhone has no Back button. Tap the app\'s back control instead.');
        }
        return await this.simple(phone, 'back');
      case 'phone.open_app':
        return await this.openApp(phone, a);
      case 'phone.read_ui':
        return await this.readUi(phone, a);
      case 'phone.screen_text':
        return await this.screenText(phone);
      case 'phone.find_and_tap':
        return await this.findAndTap(phone, a);
      case 'phone.key':
        return fail('unsupported', 'Phone-Linked phones don\'t take hardware keys. Tap the on-screen control instead.');
      case 'phone.task':
        return fail('unsupported', 'Plain-English tasks aren\'t available on a linked phone yet. Use phone.read_ui, phone.find_and_tap and the other phone tools.');
      case 'phone.sms.send':
        return fail('unsupported', 'Texts aren\'t available on a linked phone yet. Use an Android phone over adb.');
      case 'phone.call.dial':
        return fail('unsupported', 'Calls aren\'t available on a linked phone yet. Use an Android phone over adb.');
      default:
        return fail('unknown_tool', `No phone tool named ${name}.`);
    }
  }

  // ── calls ────────────────────────────────────────────────────────────────

  private async invoke<T>(phone: LinkedPhone, method: string, params: Record<string, unknown> = {}, timeoutMs?: number): Promise<{ ok: true; value: T } | { ok: false; result: ToolResult }> {
    const host = await (this.deps.resolveHost ?? defaultResolveHost)(phone);
    if (!host) {
      return { ok: false, result: fail('device_offline', `No address is known for ${phone.name ?? 'this phone'} yet. Open Phone Link setup to reconnect it.`) };
    }
    try {
      const value = await this.deps.client(phone, host).call<T>(method, params, timeoutMs);
      return { ok: true, value };
    } catch (error) {
      return { ok: false, result: await this.mapError(phone, host, error) };
    }
  }

  /** IctrlError → ToolErrorCode; anything else becomes the generic lane failure. */
  private async mapError(phone: LinkedPhone, host: string, error: unknown): Promise<ToolResult> {
    if (error instanceof IctrlError) {
      if (error.code === 'unreachable') {
        return fail('device_offline', `Can't reach ${phone.name ?? 'the phone'} at ${host}. Check the phone is on the same Wi-Fi as this computer.`);
      }
      if (error.needsRepair) {
        return fail('pairing_invalid', 'Pair this phone again in Settings › Subscriptions › Linked phones.');
      }
      // The helpers refuse screen work while the phone is locked; confirm before blaming the lane.
      const locked = await this.deps
        .client(phone, host)
        .call<{ locked?: boolean }>('is_locked')
        .then((r) => r?.locked === true)
        .catch(() => false);
      if (locked) return fail('device_locked', 'Unlock your phone and try again.');
    }
    return fail('phone_failed', error instanceof Error ? error.message : String(error));
  }

  private async simple(phone: LinkedPhone, method: string): Promise<ToolResult> {
    const res = await this.invoke(phone, method);
    return res.ok ? { ok: true } : res.result;
  }

  /**
   * The image is requested in the phone's own tap coordinates (points on
   * iPhone via scale 1, pixels on Android), so screenshot, read_ui frames and
   * tap/swipe all share one coordinate space and nothing is converted.
   */
  private async screenshot(phone: LinkedPhone): Promise<ToolResult> {
    const params = phone.platform === 'ios' ? { format: 'jpeg', scale: 1 } : { format: 'jpeg' };
    const res = await this.invoke<Record<string, unknown>>(phone, 'screenshot', params, 20_000);
    if (!res.ok) return res.result;
    const shot = res.value;
    const b64 = typeof shot.data === 'string' && shot.data.length > 0 ? shot.data : null;
    if (!b64) return fail('phone_failed', 'The phone returned a screenshot without image data.');
    const jpeg = Buffer.from(b64, 'base64');
    const size = jpegSize(jpeg);
    if (!size) return fail('phone_failed', 'The phone did not return a JPEG screenshot.');
    this.screens.set(phone.deviceId, size);
    return {
      ok: true,
      mimeType: 'image/jpeg',
      dataBase64: jpeg.toString('base64'),
      width: size.width,
      height: size.height,
      note: 'Tap coordinates are pixels of this image; phone.read_ui frames use the same coordinates.',
    };
  }

  private async tap(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    if (!isInt(a.x) || !isInt(a.y)) return fail('invalid_args', 'x and y must be non-negative integers.');
    const res = await this.invoke(phone, 'tap', { x: a.x, y: a.y });
    return res.ok ? { ok: true } : res.result;
  }

  private async swipe(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    const duration = a.durationMs === undefined ? 300 : a.durationMs;
    if (!Number.isInteger(duration) || (duration as number) < 50 || (duration as number) > 5000) {
      return fail('invalid_args', 'durationMs must be 50–5000.');
    }
    if (a.direction !== undefined) {
      if (a.direction !== 'up' && a.direction !== 'down' && a.direction !== 'left' && a.direction !== 'right') {
        return fail('invalid_args', 'direction must be up, down, left or right.');
      }
      // iCTRL's swipe starts at the screen centre and travels a third of the screen by default.
      const res = await this.invoke(phone, 'swipe', { direction: a.direction });
      return res.ok ? { ok: true } : res.result;
    }
    const coords = [a.x1, a.y1, a.x2, a.y2];
    if (!coords.every((c) => isInt(c))) return fail('invalid_args', 'Give x1,y1,x2,y2 as non-negative integers, or a direction.');
    const [x1, y1, x2, y2] = coords as number[];
    const res = await this.invoke(phone, 'drag', { from_x: x1, from_y: y1, to_x: x2, to_y: y2, duration: (duration as number) / 1000 });
    return res.ok ? { ok: true } : res.result;
  }

  private async typeText(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    if (typeof a.text !== 'string' || a.text.length === 0 || a.text.length > 500) {
      return fail('invalid_args', 'text must be 1–500 characters.');
    }
    const res = await this.invoke(phone, 'type', { text: a.text });
    return res.ok ? { ok: true } : res.result;
  }

  private appCandidates(phone: LinkedPhone, app: string): string[] {
    if (PACKAGE_RE.test(app)) return [app];
    const key = app.toLowerCase();
    // A catalog id ("google_drive") or name ("Google Drive").
    const entry = catalogApp(key) ?? catalogAppByName(key);
    const id = entry?.id ?? key;
    if (phone.platform === 'ios') return [...(IOS_VENDOR_BUNDLES[id] ?? [])];
    const vendor = ANDROID_VENDOR_PACKAGES[id];
    if (vendor) return [...vendor];
    const alias = resolveApp(key);
    return alias ? [alias] : [];
  }

  private async openApp(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    if (typeof a.app !== 'string' || !a.app.trim() || a.app.length > 128) return fail('invalid_args', 'app must be an app name or package id.');
    const app = a.app.trim();
    const candidates = this.appCandidates(phone, app);
    if (candidates.length === 0) {
      return fail('unknown_app', `"${app}" isn't a known app name. Use the app name or its package id.`);
    }
    let last: ToolResult | null = null;
    for (const bundle of candidates) {
      const res = await this.invoke(phone, 'launch_app', { bundle_id: bundle });
      if (res.ok) return { ok: true, app: bundle };
      last = res.result;
      // Transport, pairing and lock failures are not per-app; don't try the next alias.
      if (last.error === 'device_offline' || last.error === 'pairing_invalid' || last.error === 'device_locked') return last;
    }
    return fail('unknown_app', `No app matching "${app}" is installed on the phone.`);
  }

  private async readUi(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    const params = pageSourceParams(a);
    if ('error' in params) return params.error;
    const res = await this.invoke<unknown>(phone, 'page_source', params.params);
    if (!res.ok) return res.result;
    return { ok: true, text: textOf(res.value) };
  }

  private async screenText(phone: LinkedPhone): Promise<ToolResult> {
    const res = await this.invoke<unknown>(phone, 'screen_text');
    if (!res.ok) return res.result;
    return { ok: true, text: textOf(res.value) };
  }

  private async findAndTap(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    if (typeof a.element_id !== 'string' || !a.element_id.trim()) return fail('invalid_args', 'element_id is required, e.g. "sign_in" or "Sign in".');
    if (a.match !== undefined && typeof a.match !== 'string') return fail('invalid_args', 'match must be a string.');
    const res = await this.invoke<{ message?: string; matched?: unknown }>(phone, 'find_and_tap', {
      element_id: a.element_id.trim(),
      ...(typeof a.match === 'string' && a.match ? { match: a.match } : {}),
    });
    return res.ok ? { ok: true, message: res.value?.message, matched: res.value?.matched } : res.result;
  }
}

export function defaultResolveHost(phone: LinkedPhone): Promise<string | null> {
  return Promise.resolve(phone.lanHost ?? null);
}

/** page_source params shared by the read tools; validation matches the adb lane. */
function pageSourceParams(a: Record<string, unknown>): { params: Record<string, unknown> } | { error: ToolResult } {
  const params: Record<string, unknown> = { format: 'text' };
  if (a.filter !== undefined) {
    if (typeof a.filter !== 'object' || a.filter === null) return { error: fail('invalid_args', 'filter must be an object like { label, type }.') };
    const f = a.filter as { label?: unknown; type?: unknown };
    if (f.label !== undefined && typeof f.label !== 'string') return { error: fail('invalid_args', 'filter.label must be a string.') };
    if (f.type !== undefined && typeof f.type !== 'string') return { error: fail('invalid_args', 'filter.type must be a string.') };
    params.filter = a.filter;
  }
  if (a.visible_only !== undefined) {
    if (typeof a.visible_only !== 'boolean') return { error: fail('invalid_args', 'visible_only must be a boolean.') };
    params.visible_only = a.visible_only;
  }
  if (a.max_elements !== undefined) {
    if (!Number.isInteger(a.max_elements) || (a.max_elements as number) < 1 || (a.max_elements as number) > 1000) {
      return { error: fail('invalid_args', 'max_elements must be 1–1000.') };
    }
    params.max_elements = a.max_elements;
  }
  return { params };
}

/** page_source/screen_text answers: a bare string, a {text}, or JSON. */
function textOf(value: unknown): string {
  if (typeof value === 'string') return value;
  const v = (value ?? {}) as { source?: unknown; text?: unknown; items?: unknown };
  // screen_text → { items: [{ label | text, center_x, center_y }] } (Android also sends source: 'tree');
  // page_source → { source }
  if (Array.isArray(v.items)) {
    return v.items
      .map((item) => {
        const it = item as { label?: unknown; text?: unknown; center_x?: unknown; center_y?: unknown };
        const text = typeof it.label === 'string' ? it.label : typeof it.text === 'string' ? it.text : '';
        return text ? `${text} @ (${it.center_x}, ${it.center_y})` : '';
      })
      .filter(Boolean)
      .join('\n');
  }
  if (typeof v.source === 'string') return v.source;
  if (typeof v.text === 'string') return v.text;
  return JSON.stringify(value ?? {});
}

/** JPEG pixel size from the SOF marker; no decoding. */
export function jpegSize(buf: Buffer): { width: number; height: number } | null {
  if (buf.length < 4 || buf[0] !== 0xff || buf[1] !== 0xd8) return null;
  let off = 2;
  while (off + 9 < buf.length) {
    if (buf[off] !== 0xff) {
      off++;
      continue;
    }
    const marker = buf[off + 1];
    if (marker === 0xd8 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) {
      off += 2;
      continue;
    }
    if (marker === 0xd9) return null;
    const len = buf.readUInt16BE(off + 2);
    if (len < 2 || off + 2 + len > buf.length) return null;
    if ([0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf].includes(marker)) {
      return { height: buf.readUInt16BE(off + 5), width: buf.readUInt16BE(off + 7) };
    }
    off += 2 + len;
  }
  return null;
}

// ── Production source: both link stores ─────────────────────────────────────

export function linkedPhoneSource(deps: { iosStore: IosLinkStore; androidStore: AndroidLinkStore }): LinkedPhoneSource {
  return () => [
    ...deps.iosStore.all().map((r) => ({
      deviceId: r.deviceId ?? r.udid,
      platform: 'ios' as const,
      name: r.deviceName,
      lanHost: r.lanHost,
      meshHost: r.meshHost,
      storeKey: ictrlStoreKey(r.udid),
      expiresAt: r.expiresAt,
      apps: r.apps ? nameCatalogApps(r.apps) : r.apps,
    })),
    ...deps.androidStore.all().map((r) => ({
      deviceId: r.deviceId,
      platform: 'android' as const,
      name: r.deviceName,
      lanHost: r.lanHost,
      meshHost: r.meshHost,
      storeKey: ictrlStoreKey(`android:${r.deviceId}`),
      apps: r.apps ? nameCatalogApps(r.apps) : r.apps,
    })),
  ];
}

/**
 * Where a linked phone answers now. Phones change address (measured: an
 * iPhone between sessions, an Android phone after a reboot), so a stale
 * saved host falls back to a LAN scan for the device id; a new address is
 * written back to its link store. Order: saved LAN host → shared LAN scan →
 * mesh (only when the record has a meshHost and a meshProxy is wired). The
 * mesh proxy is accepted only when its /whoami reports this phone's device
 * id — a shared sidecar could be proxying a different phone. Answers are
 * cached briefly so a burst of tool calls doesn't re-probe every time.
 *
 * Returns a HostResolver: callable as today, plus `lastPath(deviceId)` for
 * phone.devices' `path: 'lan' | 'mesh'` field. The mesh proxy's ephemeral
 * loopback address is never written back to the link store.
 */
export function cachedHostResolver(deps: {
  discovery: DiscoveryDeps;
  remember: (phone: LinkedPhone, host: string) => void;
  /**
   * Bridge a saved mesh address (`<100.x>:8200`) to a local loopback host
   * (`127.0.0.1:<port>`) via the mesh-node sidecar. Production wires
   * MeshManager.proxyFor; tests fake it. Omit to disable the mesh fallback.
   */
  meshProxy?: (meshHost: string) => Promise<string | null>;
  ttlMs?: number;
  now?: () => number;
}): HostResolver {
  const cache = new Map<string, { host: string; at: number; path: 'lan' | 'mesh' }>();
  const paths = new Map<string, 'lan' | 'mesh'>();
  const now = deps.now ?? Date.now;
  const ttl = deps.ttlMs ?? 60_000;
  // One LAN scan at a time, shared by every phone that needs it (and reused for the ttl).
  let scan: { at: number; result: Promise<Map<string, string>> } | null = null;
  const sharedScan = () => {
    if (!scan || now() - scan.at >= ttl) scan = { at: now(), result: scanLan(deps.discovery) };
    return scan.result;
  };
  const resolve = async (phone: LinkedPhone): Promise<string | null> => {
    const hit = cache.get(phone.deviceId);
    if (hit && now() - hit.at < ttl) return hit.host;
    let host: string | null = null;
    let path: 'lan' | 'mesh' = 'lan';
    if (phone.lanHost) {
      const who = await deps.discovery.fetchWhoami(phone.lanHost, 1500).catch(() => null);
      if (who?.device_id === phone.deviceId) host = phone.lanHost;
    }
    if (!host) host = (await sharedScan()).get(phone.deviceId) ?? null;
    if (!host && phone.meshHost && deps.meshProxy) {
      const proxy = await deps.meshProxy(phone.meshHost).catch(() => null);
      if (proxy) {
        const who = await deps.discovery.fetchWhoami(proxy, 3000).catch(() => null);
        if (who?.device_id === phone.deviceId) {
          host = proxy;
          path = 'mesh';
        }
      }
    }
    if (!host) {
      cache.delete(phone.deviceId);
      paths.delete(phone.deviceId);
      return null;
    }
    cache.set(phone.deviceId, { host, at: now(), path });
    paths.set(phone.deviceId, path);
    if (path === 'lan' && host !== phone.lanHost) deps.remember(phone, host);
    return host;
  };
  resolve.lastPath = (deviceId: string) => paths.get(deviceId);
  return resolve;
}

/** Writes a phone's new address back to the store it came from. */
/** Saves a phone's re-read app list into its link record. */
export function rememberLinkedApps(stores: { iosStore: IosLinkStore; androidStore: AndroidLinkStore }) {
  return (phone: LinkedPhone, apps: PhoneLinkApp[]): void => {
    if (phone.platform === 'android') {
      const record = stores.androidStore.load(phone.deviceId);
      if (record) stores.androidStore.save({ ...record, apps });
      return;
    }
    const record = stores.iosStore.all().find((r) => (r.deviceId ?? r.udid) === phone.deviceId);
    if (record) stores.iosStore.save({ ...record, apps });
  };
}

export function rememberLinkedHost(stores: { iosStore: IosLinkStore; androidStore: AndroidLinkStore }) {
  return (phone: LinkedPhone, host: string): void => {
    if (phone.platform === 'android') {
      const record = stores.androidStore.load(phone.deviceId);
      if (record) stores.androidStore.save({ ...record, lanHost: host });
      return;
    }
    const record = stores.iosStore.all().find((r) => (r.deviceId ?? r.udid) === phone.deviceId);
    if (record) stores.iosStore.save({ ...record, lanHost: host });
  };
}

/**
 * The evidence probe the health monitor runs per phone: unauthenticated
 * /whoami (must match the linked device id), then a sealed call, then
 * is_locked. All failures degrade to the matching evidence, never a throw.
 */
export async function probeLinkedPhone(
  phone: LinkedPhone,
  client: (phone: LinkedPhone, host: string) => IctrlLike,
  resolveHost: (phone: LinkedPhone) => Promise<string | null> = defaultResolveHost,
): Promise<HealthEvidence> {
  const base = { deviceId: phone.deviceId, platform: phone.platform, expiresAt: phone.expiresAt };
  const host = await resolveHost(phone);
  if (!host) return { ...base, whoami: null, call: 'failed' };
  const c = client(phone, host);
  let whoami: { device_id?: string } | null = null;
  try {
    whoami = await c.whoami();
  } catch {
    whoami = null;
  }
  if (!whoami || typeof whoami.device_id !== 'string' || whoami.device_id !== phone.deviceId) {
    return { ...base, whoami: null, call: 'failed' };
  }
  try {
    await c.call('device_info', {}, 5000);
    const locked = await c.call<{ locked?: boolean }>('is_locked').catch(() => null);
    return { ...base, whoami: { deviceId: whoami.device_id }, call: locked?.locked === true ? 'locked' : 'ok' };
  } catch (error) {
    return { ...base, whoami: { deviceId: whoami.device_id }, call: error instanceof IctrlError && error.needsRepair ? 'needs_repair' : 'failed' };
  }
}
