/**
 * Phone Use route `bt-apple` (docs/contracts/CONTRACT-phone-use-2026-11-16.md
 * §1 row 2c, §5, §6, §9.2): a SECOND iPhone or iPad running the App Store
 * Allternit app, with "Be a keyboard for another iPhone" started (Settings >
 * Device Access), is the Bluetooth keyboard and mouse ("Allternit Keyboard")
 * of the person's MAIN iPhone. A main route: two Apple devices and no Mac is
 * the common setup.
 *
 *   hands: `hid.*` over the SECOND device's app-phone control socket (the
 *          node's `POST /api/v1/phones/app/:id/rpc`, the same sealed lane and
 *          pairing as everything else on it); the app turns them into
 *          Bluetooth HID reports (iOS Core/Remote/KeyboardCommands.swift,
 *          same methods and error codes as the Android sender)
 *   eyes:  the MAIN iPhone's Allternit screen broadcast (phone-app-store.ts stills)
 *   aim:   the shared keyboard/mouse core (phone-hid-core.ts)
 *
 * Which device drives which iPhone: both are `app:` iOS phones. The target is
 * the serial asked for; it is never its own sender (an app never sends input
 * to its own device). Senders are the account's other iOS app phones whose
 * keyboard mode answers `available`. A Bluetooth peripheral is not told its
 * host's name, so with several broadcasting iPhones or several connected
 * keyboards the lane can't tell who is paired with whom and says so instead
 * of guessing.
 *
 * Starting keyboard mode is the person's tap on the second device; Desktop
 * can't start it (there is no `hid.pair`). Every acting call has already
 * crossed guardCheck in phone-tools.ts before the router picks this lane.
 *
 * UNDOCUMENTED (accepted by Eoj 2026-11-17): iOS accepts the HID service only
 * under its 128-bit UUID form; if it refuses, the device answers "Not
 * available on this iPhone or iPad" and the route is unavailable with that
 * sentence. NOT CHECKED ON HARDWARE (P5, and P3/P4 for the pointer).
 */

import { BroadcastStills } from './phone-broadcast-stills.js';
import { HidCore, HidError, hidRouteHealth, senderFromTransport, type Calibration, type HidLinkStatus, type HidTransport, type Point } from './phone-hid-core.js';
import { needsYou, type RouteHealth } from './phone-link-health.js';
import type { Capability, LaneContext, LaneTool, PhoneLane } from './phone-route.js';
import type { ToolErrorCode, ToolResult } from './phone-tools.js';
import type { Gray } from './phone-usb-agent-vision.js';
import { frameOf, type VerifyFrame } from './phone-verify.js';

/** What the lane needs from the app-phone backend (AppPhoneBackend satisfies it). */
export interface BtApplePhones {
  refresh(): Promise<void>;
  /** Every paired app phone. `screen` = sharing right now. */
  devices(): Array<{ serial: string; platform: 'ios' | 'android'; name: string; screen: boolean }>;
  /** phone.screenshot of an app iPhone: a still of its broadcast. */
  call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
}

export type BtAppleRpcResult = { ok: true; result: Record<string, unknown> } | { ok: false; status: number; code: string; message: string };

export interface BtAppleDeps {
  phones: () => BtApplePhones | null;
  /** The node's app-phone rpc (`AppPhoneNode.rpc`), by phone id (the serial without `app:`). */
  rpc: (phoneId: string, method: string, params: Record<string, unknown>, timeoutMs: number) => Promise<BtAppleRpcResult>;
  /** JPEG → grayscale at about `width` px wide. */
  decode: (jpeg: Buffer, width: number) => Gray | null;
  /** Pointer calibration per target iPhone serial. */
  load: (serial: string) => Calibration | null;
  save: (serial: string, c: Calibration) => void;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

const COVERS: readonly Capability[] = ['tap', 'type', 'swipe', 'key'];
const DECODE_WIDTH = 360;
const STATUS_TIMEOUT_MS = 5_000;
const HID_TIMEOUT_MS = 15_000;
/** hid.pointer.reset sweeps 158 reports on the device. */
const RESET_TIMEOUT_MS = 20_000;
/** hid.type text per message (contract §9.2: max 500). */
const TYPE_CHUNK = 200;
const APP_PREFIX = 'app:';
/**
 * After calibration missed the pointer, health says ASSISTIVETOUCH_OFF for
 * this long, then lets the next tap look again (the router only calls act on
 * a healthy lane, so without this the route could never recover).
 */
export const POINTER_RECHECK_MS = 30_000;

export const BT_APPLE_NO_SECOND_DEVICE = 'No second iPhone or iPad with the Allternit app is connected to this account.';
export const BT_APPLE_UPDATE_APP = 'Update the Allternit app on your other iPhone or iPad to use it as a Bluetooth keyboard and mouse.';
export const BT_APPLE_OPEN_APP = 'Open the Allternit app on your other iPhone or iPad and start Be a keyboard for another iPhone (Settings > Device Access).';
export const BT_APPLE_SEVERAL_KEYBOARDS = 'More than one iPhone or iPad is acting as Allternit Keyboard. Stop it on all but one.';
export const BT_APPLE_SEVERAL_TARGETS = 'More than one iPhone is sharing its screen with Allternit, and the Allternit Keyboard can’t tell which one it is paired with. Stop sharing on the other one.';

const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

/** A failure the second device (or the node) answered. */
export class BtAppleError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** The second device's answer to hid.pair.status. */
interface SenderStatus extends HidLinkStatus {
  available: boolean;
  message?: string;
}

/** The contract's §6 transport over the second device's app-phone rpc. */
export function btAppleTransport(rpc: BtAppleDeps['rpc'], phoneId: () => string): HidTransport {
  const call = async (method: string, params: Record<string, unknown>, timeoutMs = HID_TIMEOUT_MS): Promise<Record<string, unknown>> => {
    let res: BtAppleRpcResult;
    try {
      res = await rpc(phoneId(), method, params, timeoutMs);
    } catch (error) {
      if (error instanceof BtAppleError) throw error;
      throw new BtAppleError('device_offline', `Allternit Desktop's local API didn't answer (${error instanceof Error ? error.message : String(error)}).`);
    }
    if (!res.ok) {
      const e = res as Extract<BtAppleRpcResult, { ok: false }>;
      throw new BtAppleError(e.code, e.message);
    }
    return (res as Extract<BtAppleRpcResult, { ok: true }>).result;
  };
  return {
    status: async () => readStatus(await call('hid.pair.status', {}, STATUS_TIMEOUT_MS)),
    pointerMove: async (dx, dy) => void (await call('hid.pointer.move', { dx, dy })),
    pointerClick: async (button, down = true, up = true) => void (await call('hid.pointer.click', { button, down, up })),
    pointerReset: async () => void (await call('hid.pointer.reset', {}, RESET_TIMEOUT_MS)),
    key: async (usage, modifiers, down) => void (await call('hid.key', { usage, modifiers, down })),
    typeText: async (text) => {
      for (let i = 0; i < text.length; i += TYPE_CHUNK) await call('hid.type', { text: text.slice(i, i + TYPE_CHUNK) });
    },
    scroll: async (lines) => void (await call('hid.scroll', { lines })),
  };
}

function readStatus(r: Record<string, unknown>): SenderStatus {
  return {
    paired: r.paired === true,
    connected: r.connected === true,
    available: r.available === true,
    ...(typeof r.name === 'string' && r.name ? { name: r.name } : {}),
    ...(typeof r.message === 'string' && r.message ? { message: r.message } : {}),
  };
}

/** What the lane knows about one target iPhone. */
interface IphoneState {
  core: HidCore;
  /** The second device's phone id driving it (null until one is found). */
  sender: string | null;
  /** Bumps when the sender changes or its link reconnects (phone-hid-core re-homes and lets go). */
  gen: number;
  wasConnected: boolean;
  /** The AssistiveTouch pointer: seen by calibration (true), looked for and missing (false), not looked for (null). */
  pointerSeen: boolean | null;
  missedAt: number;
  size: { width: number; height: number } | null;
}

export class BtAppleLane implements PhoneLane {
  readonly id = 'bt-apple' as const;
  readonly covers = COVERS;
  readonly verify = 'router' as const;
  private readonly iphones = new Map<string, IphoneState>();
  private readonly now: () => number;

  constructor(private readonly deps: BtAppleDeps) {
    this.now = deps.now ?? Date.now;
  }

  // ── health (contract §5) ──────────────────────────────────────────────────

  async health(ctx: LaneContext): Promise<RouteHealth> {
    const phones = this.deps.phones();
    if (!phones) return { state: 'unavailable', reason: 'Phones are not set up yet.' };
    await phones.refresh();
    const iphone = phones.devices().find((d) => d.serial === ctx.serial && d.platform === 'ios');
    if (!iphone) return { state: 'unavailable', reason: 'Bluetooth from a second iPhone or iPad drives iPhones connected with the Allternit app (its screen broadcast is how I see).' };
    const found = await this.findSender(phones, ctx.serial);
    const st = this.state(ctx.serial);
    if (st.pointerSeen === false && this.now() - st.missedAt >= POINTER_RECHECK_MS) st.pointerSeen = null;
    if (found.sender) {
      if (st.sender !== found.sender.id) {
        st.sender = found.sender.id;
        st.gen += 1;
      } else if (found.sender.status.connected && !st.wasConnected) {
        st.gen += 1; // reconnected: a gesture may have been cut off
      }
      st.wasConnected = found.sender.status.connected;
    } else {
      st.sender = null;
      st.wasConnected = false;
    }
    return hidRouteHealth(
      { link: found.sender?.status ?? null, broadcast: iphone.screen, pointerSeen: st.pointerSeen, locked: ctx.locked },
      found.reason,
    );
  }

  /**
   * The second device to send through: the one whose keyboard is connected,
   * else one with keyboard mode on and waiting (BT_NOT_PAIRED), else why none
   * can (unavailable).
   */
  private async findSender(phones: BtApplePhones, target: string): Promise<{ sender?: { id: string; status: SenderStatus }; reason: string }> {
    const ios = phones.devices().filter((d) => d.platform === 'ios');
    // Never the target itself: the app does not send input to its own device.
    const others = ios.filter((d) => d.serial !== target);
    if (others.length === 0) return { reason: BT_APPLE_NO_SECOND_DEVICE };
    const answers = await Promise.all(
      others.map(async (d) => {
        const id = d.serial.slice(APP_PREFIX.length);
        try {
          return { serial: d.serial, id, status: (await btAppleTransport(this.deps.rpc, () => id).status()) as SenderStatus, error: null };
        } catch (error) {
          return { serial: d.serial, id, status: null, error: error instanceof BtAppleError ? error : new BtAppleError('phone_failed', String(error)) };
        }
      }),
    );
    const ok = answers.filter((a): a is { serial: string; id: string; status: SenderStatus; error: null } => a.status !== null);
    const keyboards = ok.filter((a) => a.status.available);
    const connected = keyboards.filter((a) => a.status.connected);
    if (connected.length > 1) return { reason: BT_APPLE_SEVERAL_KEYBOARDS };
    if (keyboards.length > 0) {
      // Another broadcasting iPhone that is not a keyboard could be the one paired.
      const senders = new Set(keyboards.map((k) => k.serial));
      if (ios.some((d) => d.serial !== target && d.screen && !senders.has(d.serial))) return { reason: BT_APPLE_SEVERAL_TARGETS };
      const pick = connected[0] ?? keyboards[0];
      return { sender: { id: pick.id, status: pick.status }, reason: '' };
    }
    const said = ok.find((a) => a.status.message)?.status.message;
    if (said) return { reason: said };
    const err = answers.find((a) => a.error)?.error;
    if (err?.code === 'unknown_method' || err?.code === 'bad_request') return { reason: BT_APPLE_UPDATE_APP };
    if (err?.code === 'app_not_open' || err?.code === 'app_timeout') return { reason: BT_APPLE_OPEN_APP };
    return { reason: err?.message || 'Your other iPhone or iPad did not answer.' };
  }

  // ── acting ────────────────────────────────────────────────────────────────

  async act(tool: LaneTool, serial: string, a: Record<string, unknown>): Promise<ToolResult> {
    const st = this.state(serial);
    if (!st.sender) {
      const h = await this.health({ serial, locked: null });
      if (h.state === 'needs_you') return { ok: false, error: 'needs_you', message: h.needsYou.message, needs_you: h.needsYou } as ToolResult;
      if (h.state === 'unavailable') return fail('route_unavailable', h.reason);
    }
    try {
      switch (tool) {
        case 'phone.tap': {
          const p = await this.fraction(serial, a.x, a.y);
          const blocked = await this.calibrated(serial);
          if (blocked) return blocked;
          await st.core.tap(p[0], p[1]);
          return { ok: true };
        }
        case 'phone.swipe': {
          let pts: Point[];
          if (a.direction !== undefined) {
            const d = ({ up: [[0.5, 0.7], [0.5, 0.3]], down: [[0.5, 0.3], [0.5, 0.7]], left: [[0.8, 0.5], [0.2, 0.5]], right: [[0.2, 0.5], [0.8, 0.5]] } as Record<string, Point[]>)[String(a.direction)];
            if (!d) return fail('invalid_args', 'direction must be up, down, left or right.');
            pts = d;
          } else {
            pts = [await this.fraction(serial, a.x1, a.y1), await this.fraction(serial, a.x2, a.y2)];
          }
          const blocked = await this.calibrated(serial);
          if (blocked) return blocked;
          await st.core.drag(pts);
          return { ok: true };
        }
        case 'phone.type':
          if (typeof a.text !== 'string' || !a.text.length || a.text.length > 500) return fail('invalid_args', 'text must be 1–500 characters.');
          await st.core.type(a.text);
          return { ok: true };
        case 'phone.key':
          if (typeof a.combo !== 'string') return fail('invalid_args', 'combo is required, e.g. "enter" or "cmd+space".');
          await st.core.key(a.combo);
          return { ok: true };
        case 'phone.home':
          await st.core.pressHome();
          return { ok: true };
        case 'phone.back': {
          const blocked = await this.calibrated(serial);
          if (blocked) return blocked;
          await st.core.goBack();
          return { ok: true };
        }
        case 'phone.open_app':
          if (typeof a.app !== 'string' || !a.app.trim() || a.app.length > 60) return fail('invalid_args', 'app must be an app name, e.g. "Notes".');
          await st.core.openApp(a.app.trim());
          return { ok: true };
        default:
          return fail('not_supported', `${tool} does not run on the bt-apple route.`);
      }
    } catch (error) {
      return failure(error);
    }
  }

  /** One still of the iPhone for act-then-verify (and the size taps are measured against). */
  async frame(serial: string): Promise<VerifyFrame | null> {
    const shot = await this.still(serial);
    return shot ? frameOf(shot.jpeg, this.deps.decode(shot.jpeg, DECODE_WIDTH)) : null;
  }

  /** Corner reset before a retried tap (contract §4.1 step 5). */
  async rehome(serial: string): Promise<void> {
    await this.state(serial).core.reset();
  }

  // ── internals ─────────────────────────────────────────────────────────────

  private state(serial: string): IphoneState {
    const known = this.iphones.get(serial);
    if (known) return known;
    const holder = { gen: 0, sender: null as string | null };
    const transport = btAppleTransport(this.deps.rpc, () => {
      if (!holder.sender) throw new BtAppleError('route_unavailable', 'No second iPhone or iPad is connected to this iPhone by Bluetooth.');
      // Defence in depth: never drive a device through itself.
      if (APP_PREFIX + holder.sender === serial) throw new BtAppleError('route_unavailable', 'An iPhone can’t be its own Bluetooth keyboard.');
      return holder.sender;
    });
    const screen = new BroadcastStills(
      async () => {
        const s = await this.still(serial);
        return s ? { jpeg: s.jpeg, ageMs: s.ageMs } : null;
      },
      () => this.deps.phones()?.devices().find((d) => d.serial === serial)?.screen === true,
      (jpeg) => this.deps.decode(jpeg, DECODE_WIDTH),
      { now: this.deps.now, sleep: this.deps.sleep },
    );
    const core = new HidCore({
      sender: senderFromTransport('bt-apple', transport, { linkGeneration: () => holder.gen }),
      screen,
      load: () => this.deps.load(serial),
      save: (c) => this.deps.save(serial, c),
      sleep: this.deps.sleep,
      now: this.deps.now,
    });
    // The state object and the transport share sender/gen through `holder`.
    const st = Object.assign(holder, { core, wasConnected: false, pointerSeen: null, missedAt: 0, size: null }) as IphoneState;
    this.iphones.set(serial, st);
    return st;
  }

  private async still(serial: string): Promise<{ jpeg: Buffer; ageMs: number; width: number; height: number } | null> {
    const res = await this.deps.phones()?.call('phone.screenshot', serial, {});
    if (!res?.ok || typeof res.dataBase64 !== 'string') return null;
    const width = Number(res.width);
    const height = Number(res.height);
    if (!(width > 0 && height > 0)) return null;
    const st = this.iphones.get(serial);
    if (st) st.size = { width, height };
    return { jpeg: Buffer.from(res.dataBase64, 'base64'), ageMs: Number(res.ageMs) || 0, width, height };
  }

  /** Pixels of the latest still → screen fractions. */
  private async fraction(serial: string, x: unknown, y: unknown): Promise<Point> {
    const isCoord = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0 && (v as number) <= 20_000;
    if (!isCoord(x) || !isCoord(y)) throw new HidError('bad_params', 'Coordinates must be non-negative integers from phone.screenshot.');
    const size = this.state(serial).size ?? (await this.still(serial)) ?? null;
    if (!size) throw new BtAppleError('no_screen', needsYou('BROADCAST_STOPPED').message);
    if (x >= size.width || y >= size.height) throw new HidError('bad_params', `Coordinates are outside the ${size.width}×${size.height} screenshot.`);
    return [(x + 0.5) / size.width, (y + 0.5) / size.height];
  }

  /** Calibrates on first pointer use. Not found = the AssistiveTouch pointer is not showing. */
  private async calibrated(serial: string): Promise<ToolResult | null> {
    const st = this.state(serial);
    if (st.core.calibrated && st.pointerSeen !== false) return null;
    const ok = await st.core.calibrate();
    st.pointerSeen = ok;
    if (ok) return null;
    st.missedAt = this.now();
    const need = needsYou('ASSISTIVETOUCH_OFF');
    return { ok: false, error: 'needs_you', message: need.message, needs_you: need } as ToolResult;
  }
}

/** A thrown failure → the tool result; a keyboard with no connected iPhone is BT_NOT_PAIRED. */
function failure(error: unknown): ToolResult {
  if (error instanceof HidError) {
    const map: Record<HidError['code'], ToolErrorCode> = { bad_params: 'invalid_args', not_calibrated: 'not_calibrated', unsupported: 'unsupported' };
    return fail(map[error.code], error.message);
  }
  if (error instanceof BtAppleError) {
    if (error.code === 'bt_not_paired') {
      const need = needsYou('BT_NOT_PAIRED');
      return { ok: false, error: 'needs_you', message: need.message, needs_you: need } as ToolResult;
    }
    const map: Record<string, ToolErrorCode> = {
      bad_params: 'invalid_args',
      invalid_params: 'invalid_args',
      app_not_open: 'device_offline',
      app_timeout: 'device_offline',
      device_offline: 'device_offline',
      no_screen: 'no_screen',
      route_unavailable: 'route_unavailable',
      hid_unavailable: 'route_unavailable',
      unknown_method: 'not_supported',
    };
    return fail(map[error.code] ?? 'phone_failed', error.message);
  }
  return fail('phone_failed', error instanceof Error ? error.message : String(error));
}

/** The `bt-apple` lane for the router (registered in phone-service.ts). */
export function btAppleLane(deps: BtAppleDeps): BtAppleLane {
  return new BtAppleLane(deps);
}
