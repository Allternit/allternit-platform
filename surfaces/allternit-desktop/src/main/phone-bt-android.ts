/**
 * Phone Use route `bt-android` (docs/contracts/CONTRACT-phone-use-2026-11-16.md
 * §1 row 2d, §6, §9.2): the person's own Android phone, running the Allternit
 * app, is a Bluetooth keyboard and mouse for their own iPhone.
 *
 *   hands: `hid.*` over the app-phone control socket of the ANDROID phone
 *          (the node's `POST /api/v1/phones/app/:id/rpc`, the same sealed lane
 *          and pairing as everything else on it); the app turns them into
 *          Bluetooth HID reports (BluetoothRemote / HidCommands in the app)
 *   eyes:  the IPHONE's Allternit screen broadcast (phone-app-store.ts stills)
 *   aim:   the shared keyboard/mouse core (phone-hid-core.ts): calibration
 *          against the AssistiveTouch pointer, corner re-homing, Spotlight
 *
 * The lane serves App Store iPhones (`app:` serials, platform ios) only, since
 * the broadcast is its only way to see. Every acting call has already crossed
 * guardCheck in phone-tools.ts before the router picks this lane.
 *
 * Which Android phone drives which iPhone: the Android phone whose Bluetooth
 * keyboard is connected right now, preferring one whose connected host has the
 * iPhone's name. With one iPhone (the setup this route is for) that is exact.
 */

import { HidCore, HidError, hidRouteHealth, senderFromTransport, type Calibration, type HidLinkStatus, type HidTransport, type Point, type PointerScreen } from './phone-hid-core.js';
import { needsYou, type RouteHealth } from './phone-link-health.js';
import type { Capability, LaneContext, LaneTool, PhoneLane } from './phone-route.js';
import type { ToolErrorCode, ToolResult } from './phone-tools.js';
import { changed, type Gray } from './phone-usb-agent-vision.js';
import { frameOf, type VerifyFrame } from './phone-verify.js';

/** What the lane needs from the app-phone backend (AppPhoneBackend satisfies it). */
export interface BtAndroidPhones {
  /** Re-read the node's phone list (the backend caches it a few seconds). */
  refresh(): Promise<void>;
  /** Every paired app phone, Android and iPhone. `screen` = sharing right now. */
  devices(): Array<{ serial: string; platform: 'ios' | 'android'; name: string; screen: boolean }>;
  /** phone.screenshot of an app iPhone: a still of its broadcast. */
  call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
}

export type BtRpcResult = { ok: true; result: Record<string, unknown> } | { ok: false; status: number; code: string; message: string };

export interface BtAndroidDeps {
  phones: () => BtAndroidPhones | null;
  /** The node's app-phone rpc (`AppPhoneNode.rpc`), by phone id (the serial without `app:`). */
  rpc: (phoneId: string, method: string, params: Record<string, unknown>, timeoutMs: number) => Promise<BtRpcResult>;
  /** JPEG → grayscale at about `width` px wide. */
  decode: (jpeg: Buffer, width: number) => Gray | null;
  /** Pointer calibration per iPhone serial. */
  load: (serial: string) => Calibration | null;
  save: (serial: string, c: Calibration) => void;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

const COVERS: readonly Capability[] = ['tap', 'type', 'swipe', 'key'];
const DECODE_WIDTH = 360;
const STATUS_TIMEOUT_MS = 5_000;
const HID_TIMEOUT_MS = 15_000;
/** hid.pointer.reset sweeps ~158 reports on the phone. */
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

const fail = (error: ToolErrorCode, message: string): ToolResult => ({ ok: false, error, message });

/** A failure the Android app (or the node) answered. */
export class BtAndroidError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** The Android app's answer to hid.pair.status. */
interface SenderStatus extends HidLinkStatus {
  available: boolean;
  message?: string;
}

/** The contract's §6 transport over the Android phone's app-phone rpc. */
export function rpcTransport(rpc: BtAndroidDeps['rpc'], phoneId: () => string): HidTransport {
  const call = async (method: string, params: Record<string, unknown>, timeoutMs = HID_TIMEOUT_MS): Promise<Record<string, unknown>> => {
    let res: BtRpcResult;
    try {
      res = await rpc(phoneId(), method, params, timeoutMs);
    } catch (error) {
      throw new BtAndroidError('device_offline', `Allternit Desktop's local API didn't answer (${error instanceof Error ? error.message : String(error)}).`);
    }
    if (!res.ok) throw new BtAndroidError(res.code, res.message);
    return res.result;
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

/** What the lane knows about one iPhone. */
interface IphoneState {
  core: HidCore;
  /** The Android phone id driving it (null until one is found). */
  sender: string | null;
  /** Bumps when the sender changes or its link reconnects (phone-hid-core re-homes and lets go). */
  gen: number;
  wasConnected: boolean;
  /** The AssistiveTouch pointer: seen by calibration (true), looked for and missing (false), not looked for (null). */
  pointerSeen: boolean | null;
  /** When calibration last missed the pointer. */
  missedAt: number;
  /** Size of the latest still, for pixel → fraction. */
  size: { width: number; height: number } | null;
}

/** The phone's broadcast as the core needs it: stills pulled on demand and kept as grayscale. */
export class BroadcastScreen implements PointerScreen {
  private gray: Gray | null = null;
  private at: number | null = null;
  private readonly now: () => number;
  private readonly sleep: (ms: number) => Promise<void>;

  constructor(
    private readonly grab: () => Promise<{ jpeg: Buffer; ageMs: number; width: number; height: number } | null>,
    private readonly isLive: () => boolean,
    private readonly decode: (jpeg: Buffer) => Gray | null,
    opts: { now?: () => number; sleep?: (ms: number) => Promise<void> } = {},
  ) {
    this.now = opts.now ?? Date.now;
    this.sleep = opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  }

  live(): boolean {
    return this.isLive();
  }

  latestAt(): number | null {
    return this.at;
  }

  latestGray(): Gray | null {
    return this.gray;
  }

  /** Pulls a still; true when it is newer than the one kept. */
  async pull(): Promise<boolean> {
    const shot = await this.grab();
    if (!shot) return false;
    const at = this.now() - Math.max(0, shot.ageMs);
    if (this.at !== null && at <= this.at) return false;
    this.gray = this.decode(shot.jpeg);
    this.at = at;
    return true;
  }

  async nextGray(since: number, timeoutMs: number): Promise<Gray | null> {
    const deadline = this.now() + timeoutMs;
    for (;;) {
      await this.pull();
      if (this.at !== null && this.at > since) return this.gray;
      if (this.now() >= deadline) return null;
      await this.sleep(100);
    }
  }

  /** Waits at least `leastMs`, then until two stills `quietMs` apart match, or `mostMs`. */
  async settled(quietMs: number, leastMs: number, mostMs: number): Promise<void> {
    const deadline = this.now() + mostMs;
    await this.sleep(leastMs);
    await this.pull();
    while (this.now() < deadline) {
      const before = this.gray;
      await this.sleep(quietMs);
      await this.pull();
      if (before && this.gray && !changed(before, this.gray)) return;
    }
  }
}

export class BtAndroidLane implements PhoneLane {
  readonly id = 'bt-android' as const;
  readonly covers = COVERS;
  readonly verify = 'router' as const;
  private readonly iphones = new Map<string, IphoneState>();
  private readonly screens = new Map<string, BroadcastScreen>();
  private readonly now: () => number;

  constructor(private readonly deps: BtAndroidDeps) {
    this.now = deps.now ?? Date.now;
  }

  // ── health (contract §5) ──────────────────────────────────────────────────

  async health(ctx: LaneContext): Promise<RouteHealth> {
    const phones = this.deps.phones();
    if (!phones) return { state: 'unavailable', reason: 'Phones are not set up yet.' };
    await phones.refresh();
    const iphone = phones.devices().find((d) => d.serial === ctx.serial && d.platform === 'ios');
    if (!iphone) return { state: 'unavailable', reason: 'Bluetooth from Android drives iPhones connected with the Allternit app.' };
    const found = await this.findSender(phones, iphone.name);
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
    }
    return hidRouteHealth(
      { link: found.sender?.status ?? found.waiting ?? null, broadcast: iphone.screen, pointerSeen: st.pointerSeen, locked: ctx.locked },
      found.reason,
    );
  }

  /**
   * The Android phone to send through: a connected one (by the iPhone's name
   * when several are), else a ready one waiting for its iPhone (BT_NOT_PAIRED),
   * else why none can (unavailable).
   */
  private async findSender(
    phones: BtAndroidPhones,
    iphoneName: string,
  ): Promise<{ sender?: { id: string; status: SenderStatus }; waiting?: HidLinkStatus; reason: string }> {
    const androids = phones.devices().filter((d) => d.platform === 'android');
    if (androids.length === 0) return { reason: 'No Android phone with the Allternit app is connected to this account.' };
    const answers = await Promise.all(
      androids.map(async (d) => {
        const id = d.serial.slice(APP_PREFIX.length);
        try {
          return { id, status: (await rpcTransport(this.deps.rpc, () => id).status()) as SenderStatus, error: null };
        } catch (error) {
          return { id, status: null, error: error instanceof BtAndroidError ? error : new BtAndroidError('phone_failed', String(error)) };
        }
      }),
    );
    const ok = answers.filter((a): a is { id: string; status: SenderStatus; error: null } => a.status !== null);
    const connected = ok.filter((a) => a.status.available && a.status.connected);
    const want = iphoneName.trim().toLowerCase();
    const pick = connected.find((a) => (a.status.name ?? '').trim().toLowerCase() === want) ?? connected[0];
    if (pick) return { sender: pick, reason: '' };
    const ready = ok.find((a) => a.status.available);
    if (ready) return { sender: ready, waiting: ready.status, reason: '' };
    const said = ok.find((a) => a.status.message)?.status.message;
    if (said) return { reason: said };
    const err = answers.find((a) => a.error)?.error;
    if (err?.code === 'unknown_method') return { reason: 'Update the Allternit app on your Android phone to use it as a Bluetooth keyboard and mouse.' };
    if (err?.code === 'app_not_open' || err?.code === 'app_timeout') return { reason: 'Open the Allternit app on your Android phone, or turn on Control my iPhone by Bluetooth so it stays connected.' };
    return { reason: err?.message ?? 'The Android phone did not answer.' };
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
          return fail('not_supported', `${tool} does not run on the bt-android route.`);
      }
    } catch (error) {
      return this.failure(error);
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
    let st = this.iphones.get(serial);
    if (st) return st;
    const holder = { gen: 0, sender: null as string | null };
    const transport = rpcTransport(this.deps.rpc, () => {
      if (!holder.sender) throw new BtAndroidError('route_unavailable', 'No Android phone is connected to this iPhone by Bluetooth.');
      return holder.sender;
    });
    const core = new HidCore({
      sender: senderFromTransport('bt-android', transport, { linkGeneration: () => holder.gen }),
      screen: this.screen(serial),
      load: () => this.deps.load(serial),
      save: (c) => this.deps.save(serial, c),
      sleep: this.deps.sleep,
      now: this.deps.now,
    });
    // The state object and the transport share sender/gen through `holder`.
    st = Object.assign(holder, { core, wasConnected: false, pointerSeen: null, missedAt: 0, size: null }) as IphoneState;
    this.iphones.set(serial, st);
    return st;
  }

  private screen(serial: string): BroadcastScreen {
    let s = this.screens.get(serial);
    if (s) return s;
    s = new BroadcastScreen(
      () => this.still(serial),
      () => this.deps.phones()?.devices().find((d) => d.serial === serial)?.screen === true,
      (jpeg) => this.deps.decode(jpeg, DECODE_WIDTH),
      { now: this.deps.now, sleep: this.deps.sleep },
    );
    this.screens.set(serial, s);
    return s;
  }

  private async still(serial: string): Promise<{ jpeg: Buffer; ageMs: number; width: number; height: number } | null> {
    const res = await this.deps.phones()?.call('phone.screenshot', serial, {});
    if (!res?.ok || typeof res.dataBase64 !== 'string') return null;
    const width = Number(res.width);
    const height = Number(res.height);
    if (!(width > 0 && height > 0)) return null;
    this.state(serial).size = { width, height };
    return { jpeg: Buffer.from(res.dataBase64, 'base64'), ageMs: Number(res.ageMs) || 0, width, height };
  }

  /** Pixels of the latest still → screen fractions. */
  private async fraction(serial: string, x: unknown, y: unknown): Promise<Point> {
    const isCoord = (v: unknown): v is number => Number.isInteger(v) && (v as number) >= 0 && (v as number) <= 20_000;
    if (!isCoord(x) || !isCoord(y)) throw new HidError('bad_params', 'Coordinates must be non-negative integers from phone.screenshot.');
    const size = this.state(serial).size ?? ((await this.still(serial)) && this.state(serial).size);
    if (!size) throw new BtAndroidError('no_screen', needsYou('BROADCAST_STOPPED').message);
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

  private failure(error: unknown): ToolResult {
    if (error instanceof HidError) {
      const map: Record<HidError['code'], ToolErrorCode> = { bad_params: 'invalid_args', not_calibrated: 'not_calibrated', unsupported: 'unsupported' };
      return fail(map[error.code], error.message);
    }
    if (error instanceof BtAndroidError) {
      if (error.code === 'bt_not_paired') {
        const need = needsYou('BT_NOT_PAIRED');
        return { ok: false, error: 'needs_you', message: need.message, needs_you: need } as ToolResult;
      }
      const map: Record<string, ToolErrorCode> = {
        bad_params: 'invalid_args',
        invalid_params: 'invalid_args',
        app_not_open: 'device_offline',
        device_offline: 'device_offline',
        no_screen: 'no_screen',
        route_unavailable: 'route_unavailable',
        unknown_method: 'not_supported',
      };
      return fail(map[error.code] ?? 'phone_failed', error.message);
    }
    return fail('phone_failed', error instanceof Error ? error.message : String(error));
  }
}

/** The `bt-android` lane for the router (registered in phone-service.ts). */
export function btAndroidLane(deps: BtAndroidDeps): BtAndroidLane {
  return new BtAndroidLane(deps);
}
