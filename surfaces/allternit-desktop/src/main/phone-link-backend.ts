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

import * as fs from 'node:fs';
import { IctrlError, ictrlStoreKey } from './ictrl-client.js';
import { IOS_VENDOR_BUNDLES, fileLinkStore, type IctrlLike, type IosLinkStore } from './phone-link-ios.js';
import { ANDROID_VENDOR_PACKAGES } from './phone-link.js';
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
}

export const LINK_SERIAL_PREFIX = 'link:';

const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

const COORD_MAX = 20_000;
const isInt = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0 && (v as number) <= COORD_MAX;

export interface PhoneLinkBackendDeps {
  phones: LinkedPhoneSource;
  /** Client for this phone's key (secret store entry per phone) at `host`. */
  client: (phone: LinkedPhone, host: string) => IctrlLike;
  /** Where the helper answers; default is the phone's saved lanHost. The LAN-scanning findPhoneHost lands on a sibling branch — this is its seam. */
  resolveHost?: (phone: LinkedPhone) => Promise<string | null>;
  health?: PhoneLinkHealthMonitor;
  now?: () => number;
  logger?: (message: string) => void;
}

export class PhoneLinkBackend implements UsbPhoneBackend {
  /** image pixels per phone point, learned from a screenshot (deviceId → scale). */
  private readonly scales = new Map<string, number>();
  /** Latest screenshot's pixel size per phone (for direction swipes). */
  private readonly screens = new Map<string, { width: number; height: number }>();

  constructor(private readonly deps: PhoneLinkBackendDeps) {}

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
        return fail('pairing_invalid', 'Pair this phone again in Settings › Computers › Phones.');
      }
      // The helpers refuse screen work while the phone is locked; confirm before blaming the lane.
      const locked = await this.deps
        .client(phone, host)
        .call<{ locked?: boolean }>('is_locked')
        .then((r) => r?.locked === true)
        .catch(() => false);
      if (locked) return fail('device_locked', 'Unlock your phone and try again.');
    }
    return fail('adb_failed', error instanceof Error ? error.message : String(error));
  }

  private async simple(phone: LinkedPhone, method: string): Promise<ToolResult> {
    const res = await this.invoke(phone, method);
    return res.ok ? { ok: true } : res.result;
  }

  private async screenshot(phone: LinkedPhone): Promise<ToolResult> {
    const res = await this.invoke<Record<string, unknown>>(phone, 'screenshot', { format: 'jpeg' }, 20_000);
    if (!res.ok) return res.result;
    const shot = res.value;
    const b64 = [shot.image, shot.image_base64, shot.data].find((v): v is string => typeof v === 'string' && v.length > 0);
    if (!b64) return fail('adb_failed', 'The phone returned a screenshot without image data.');
    const jpeg = Buffer.from(b64, 'base64');
    if (jpeg.length < 4 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8) return fail('adb_failed', 'The phone did not return a JPEG screenshot.');
    const size = jpegSize(jpeg);
    if (size) this.screens.set(phone.deviceId, size);

    // Learn the pixel↔point scale: the helper reports its screenshot size in
    // its own tap space (points on iOS); the JPEG itself is pixels.
    const space = shot.coordinate_space === 'points' || shot.coordinate_space === 'pixels' ? shot.coordinate_space : phone.platform === 'ios' ? 'points' : 'pixels';
    const reportedW = typeof shot.width === 'number' ? shot.width : undefined;
    const reportedH = typeof shot.height === 'number' ? shot.height : undefined;
    if (space === 'points' && size && reportedW && reportedH) {
      const sx = size.width / reportedW;
      const sy = size.height / reportedH;
      if (Math.abs(sx - sy) < 0.01 && Math.abs(sx - 1) > 0.01) {
        this.scales.set(phone.deviceId, sx);
      } else {
        this.scales.delete(phone.deviceId);
      }
    } else {
      this.scales.delete(phone.deviceId);
    }

    // Taps use the image's pixels whenever we can convert to the phone's
    // space; without a scale factor they pass through in the phone's space.
    const coordinateSpace = space === 'points' && !this.scales.has(phone.deviceId) ? 'points' : 'pixels';
    return {
      ok: true,
      mimeType: 'image/jpeg',
      dataBase64: jpeg.toString('base64'),
      width: size?.width ?? reportedW ?? 0,
      height: size?.height ?? reportedH ?? 0,
      coordinate_space: coordinateSpace,
      note: coordinateSpace === 'points' ? 'Tap coordinates are points, matching this image.' : 'Tap coordinates are pixels of this image.',
    };
  }

  /** Bot-space (screenshot pixels) → phone space (points when a scale is known). */
  private toPhoneCoords(phone: LinkedPhone, x: number, y: number): [number, number] {
    const scale = this.scales.get(phone.deviceId);
    return scale ? [Math.round(x / scale), Math.round(y / scale)] : [x, y];
  }

  private async tap(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    if (!isInt(a.x) || !isInt(a.y)) return fail('invalid_args', 'x and y must be non-negative integers.');
    const [x, y] = this.toPhoneCoords(phone, a.x, a.y);
    const res = await this.invoke(phone, 'tap', { x, y });
    return res.ok ? { ok: true } : res.result;
  }

  private async swipe(phone: LinkedPhone, a: Record<string, unknown>): Promise<ToolResult> {
    const duration = a.durationMs === undefined ? 300 : a.durationMs;
    if (!Number.isInteger(duration) || (duration as number) < 50 || (duration as number) > 5000) {
      return fail('invalid_args', 'durationMs must be 50–5000.');
    }
    let coords: number[];
    if (a.direction !== undefined) {
      const screen = this.screens.get(phone.deviceId);
      if (!screen) return fail('adb_failed', 'Take a screenshot before swiping by direction.');
      const [w, h] = [screen.width, screen.height];
      const [cx, cy] = [Math.floor(w / 2), Math.floor(h / 2)];
      const dx = Math.floor(w / 3);
      const dy = Math.floor(h / 3);
      const map: Record<string, number[]> = {
        up: [cx, cy + dy, cx, cy - dy],
        down: [cx, cy - dy, cx, cy + dy],
        left: [cx + dx, cy, cx - dx, cy],
        right: [cx - dx, cy, cx + dx, cy],
      };
      const picked = typeof a.direction === 'string' ? map[a.direction] : undefined;
      if (!picked) return fail('invalid_args', 'direction must be up, down, left or right.');
      coords = picked;
    } else {
      coords = [a.x1, a.y1, a.x2, a.y2] as number[];
      if (!coords.every((c) => isInt(c))) return fail('invalid_args', 'Give x1,y1,x2,y2 as non-negative integers, or a direction.');
    }
    const [x1, y1] = this.toPhoneCoords(phone, coords[0], coords[1]);
    const [x2, y2] = this.toPhoneCoords(phone, coords[2], coords[3]);
    const res = await this.invoke(phone, 'swipe', { x1, y1, x2, y2, duration_ms: duration });
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
    if (phone.platform === 'ios') return IOS_VENDOR_BUNDLES[key] ?? [];
    const vendor = ANDROID_VENDOR_PACKAGES[key];
    if (vendor) return [vendor];
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
    const res = await this.invoke(phone, 'find_and_tap', {
      element_id: a.element_id.trim(),
      ...(typeof a.match === 'string' && a.match ? { match: a.match } : {}),
    });
    return res.ok ? { ok: true } : res.result;
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
  if (value && typeof value === 'object' && typeof (value as { text?: unknown }).text === 'string') return (value as { text: string }).text;
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

/** Android link record (written by the Android Phone Link lane; read defensively). */
export interface AndroidLinkRecord {
  deviceId: string;
  serial?: string;
  deviceName?: string;
  osVersion?: string;
  lanHost?: string;
  apps?: { vendor: string; signedIn: boolean | null }[];
  pairedAt?: string;
  helperVersion?: string;
}

/**
 * Linked phones from both stores: iOS `phone-link-ios.json` (fileLinkStore)
 * and Android `phone-link-android.json`. A missing or unreadable Android file
 * means no Android phones, never an error.
 */
export function linkedPhoneSource(deps: { iosStore: IosLinkStore; androidFile: string }): LinkedPhoneSource {
  return () => {
    const ios = deps.iosStore.all().map((r) => ({
      deviceId: r.deviceId ?? r.udid,
      platform: 'ios' as const,
      name: r.deviceName,
      lanHost: r.lanHost,
      storeKey: ictrlStoreKey(r.udid),
      expiresAt: r.expiresAt,
      apps: r.apps,
    }));
    let android: AndroidLinkRecord[] = [];
    try {
      const parsed = JSON.parse(fs.readFileSync(deps.androidFile, 'utf8')) as Record<string, AndroidLinkRecord>;
      android = Object.values(parsed).filter((r) => r && typeof r.deviceId === 'string');
    } catch {
      android = [];
    }
    return [
      ...ios,
      ...android.map((r) => ({
        deviceId: r.deviceId,
        platform: 'android' as const,
        name: r.deviceName,
        lanHost: r.lanHost,
        storeKey: ictrlStoreKey(`android:${r.deviceId}`),
        apps: r.apps,
      })),
    ];
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
