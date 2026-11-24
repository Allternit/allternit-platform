/**
 * Phone Use keyboard/mouse core (docs/contracts/CONTRACT-phone-use-2026-11-16.md §6).
 *
 * One core, four senders: the Mac (`bt-mac`), the Allternit Android app
 * (`bt-android`), a second iPhone or iPad (`bt-apple`) and the USB Agent board
 * (`usb-agent`). The phone sees an ordinary keyboard and mouse; iOS
 * AssistiveTouch turns the mouse into an on-screen pointer, and the screen
 * (the app's broadcast, or AirPlay for the board) is how we see where it went.
 * Senders implement transport only (HidSender); aiming, gestures, keys and
 * checking the screen live here once.
 *
 * Pointer logic ported from esp32-iphone-agent's phone.py (MIT, Henry Yuan;
 * see hardware/usb-agent/NOTICE). A mouse is relative, so we track where the
 * pointer is in mouse units and re-home it (drive it into the top-left corner)
 * every REHOME_EVERY gestures to cancel drift. Calibration finds the pointer
 * on the screen at two known distances from the corner; after that, every
 * aimed tap checks where the pointer really is, fixes a miss and learns from
 * it. Coordinates are fractions of the screen (0..1, origin top-left).
 */

import { needsYou, type RouteHealth } from './phone-link-health.js';
import { FKA_KEYS, HOME_COMBO, parseCombo, spotlightOpenSteps, untypable, type FkaStep, type KeyStroke } from './phone-hid-keys.js';
import { changed, findPointer, type Gray } from './phone-usb-agent-vision.js';

export const CALIBRATION_STOPS = [150, 350] as const;
export const REHOME_EVERY = 10;
export const NUDGE = 12;
export const AIM_TOLERANCE = 0.008;
export const MAX_MISS = 0.06;
const HOLD_MS = 250;
const TAP_GAP_MS = 150;
const MAX_SEEN = 12;

/** A Bluetooth mouse report carries a signed byte per axis (contract §9.2: -127..127 per message). */
export const BT_MAX_STEP = 127;
/**
 * How far a corner reset drives the pointer up and left, in mouse units: more
 * than any calibrated screen span we accept (Calibration w/h ≤ 20 000), so the
 * pointer ends pinned in the corner wherever it started.
 */
export const CORNER_REACH = 20_000;

export type Point = [number, number];

export interface Calibration {
  /** Mouse units to cross the full screen width / height. */
  w: number;
  h: number;
  seen: { x: Array<[number, number]>; y: Array<[number, number]> };
}

/** The phone's screen as the core needs it (the broadcast, or AirPlay for the board). */
export interface PointerScreen {
  /** Frames are arriving right now. */
  live(): boolean;
  latestAt(): number | null;
  /** Grayscale of the newest frame. */
  latestGray(): Gray | null;
  /** Grayscale of the first frame after `since`, or null. */
  nextGray(since: number, timeoutMs: number): Promise<Gray | null>;
  settled(quietMs: number, leastMs: number, mostMs: number): Promise<void>;
}

// ── Senders ─────────────────────────────────────────────────────────────────

export type HidSenderId = 'bt-mac' | 'bt-android' | 'bt-apple' | 'usb-agent';

/** A sender's link to the phone. A cable (the board) is always paired. */
export interface HidLinkStatus {
  paired: boolean;
  connected: boolean;
  name?: string;
}

/**
 * What one sender implements: transport only. Every method sends now and
 * throws on failure (the core then forgets where the pointer is).
 */
export interface HidSender {
  readonly id: HidSenderId;
  /** The largest relative move one pointerMove may carry per axis; the core splits longer moves. */
  readonly maxStep: number;
  /** Bumps whenever the link reconnects: a new link may have dropped a gesture mid-way. */
  linkGeneration(): number;
  status(): Promise<HidLinkStatus>;
  /** Relative move in mouse units, |dx| and |dy| ≤ maxStep. */
  pointerMove(dx: number, dy: number): Promise<void>;
  /** Left button: a full click, or press / release for a drag. */
  pointerButton(action: 'click' | 'press' | 'release'): Promise<void>;
  /** Drive the pointer into the top-left corner (contract §6). */
  pointerReset(): Promise<void>;
  /** Wheel lines, positive = down. */
  scroll(lines: number): Promise<void>;
  /** Press and release one key with its modifiers. */
  key(stroke: KeyStroke): Promise<void>;
  /** Printable ASCII; the core has already checked the layout can send it. */
  typeText(text: string): Promise<void>;
  /** Let go of every button and key. */
  releaseAll(): Promise<void>;
}

/**
 * The contract's lower-level transport (§6), as the Android app and a second
 * Apple device speak it over the app-phone lane (§9.2): usages with key
 * down/up, a button with down/up. `scroll` is optional there; without it the
 * sender reports scroll as unsupported.
 */
export interface HidTransport {
  status(): Promise<HidLinkStatus>;
  pointerMove(dx: number, dy: number): Promise<void>;
  pointerClick(button: 'left' | 'right', down?: boolean, up?: boolean): Promise<void>;
  pointerReset(): Promise<void>;
  key(usage: number, modifiers: number, down: boolean): Promise<void>;
  typeText(text: string): Promise<void>;
  scroll?(lines: number): Promise<void>;
}

/** Wraps a contract transport as a sender. `linkGeneration` comes from whoever watches the link. */
export function senderFromTransport(
  id: HidSenderId,
  t: HidTransport,
  opts: { maxStep?: number; linkGeneration?: () => number } = {},
): HidSender {
  return {
    id,
    maxStep: opts.maxStep ?? BT_MAX_STEP,
    linkGeneration: opts.linkGeneration ?? (() => 0),
    status: () => t.status(),
    pointerMove: (dx, dy) => t.pointerMove(dx, dy),
    pointerButton: (action) =>
      action === 'click' ? t.pointerClick('left') : action === 'press' ? t.pointerClick('left', true, false) : t.pointerClick('left', false, true),
    pointerReset: () => t.pointerReset(),
    scroll: async (lines) => {
      if (!t.scroll) throw new HidError('unsupported', 'This keyboard and mouse cannot scroll; swipe instead.');
      await t.scroll(lines);
    },
    key: async (s) => {
      await t.key(s.usage, s.modifiers, true);
      await t.key(s.usage, s.modifiers, false);
    },
    typeText: (text) => t.typeText(text),
    releaseAll: async () => {
      await t.pointerClick('left', false, true);
      await t.key(0, 0, false);
    },
  };
}

/** A failure the core raises itself. Sender failures pass through unchanged. */
export type HidErrorCode = 'not_calibrated' | 'bad_params' | 'unsupported';
export class HidError extends Error {
  constructor(
    readonly code: HidErrorCode,
    message: string,
  ) {
    super(message);
  }
}

// ── Step sizing and the corner ──────────────────────────────────────────────

/**
 * One relative move as steps a sender accepts: every step within ±maxStep on
 * both axes, the steps adding up exactly to (dx, dy), spread evenly so the
 * pointer travels a straight line. A zero move is one zero step.
 */
export function splitMove(dx: number, dy: number, maxStep: number): Point[] {
  if (!Number.isInteger(dx) || !Number.isInteger(dy)) throw new Error('moves are whole mouse units');
  if (!(maxStep >= 1)) throw new Error('maxStep must be at least 1');
  const n = Math.max(1, Math.ceil(Math.abs(dx) / maxStep), Math.ceil(Math.abs(dy) / maxStep));
  const out: Point[] = [];
  let sx = 0;
  let sy = 0;
  for (let i = 1; i <= n; i += 1) {
    const x = Math.round((dx * i) / n);
    const y = Math.round((dy * i) / n);
    out.push([x - sx, y - sy]);
    sx = x;
    sy = y;
  }
  return out;
}

/** The steps that drive a pointer into the top-left corner from anywhere: for senders with no native "home". */
export function cornerSweep(maxStep: number, reach = CORNER_REACH): Point[] {
  return splitMove(-reach, -reach, maxStep);
}

// ── Route health from a sender's state (contract §5) ────────────────────────

export interface HidHealthInput {
  /** The sender's link, or null when this sender is not set up for the phone. */
  link: HidLinkStatus | null;
  /** Frames from the phone's broadcast are arriving. */
  broadcast: boolean;
  /** The AssistiveTouch pointer was seen (true), looked for and not seen (false), or not looked for yet (null). */
  pointerSeen: boolean | null;
  /** The phone's lock state; a keyboard and mouse need it unlocked. */
  locked: boolean | null;
}

/**
 * A Bluetooth route's health, most blocking first: not set up → unavailable;
 * not paired or not connected → BT_NOT_PAIRED; locked → unavailable (iPhone
 * Mirroring serves a locked phone); no frames → BROADCAST_STOPPED; pointer
 * looked for and missing → ASSISTIVETOUCH_OFF.
 */
export function hidRouteHealth(input: HidHealthInput, notSetUp = 'No Bluetooth keyboard and mouse is set up for this phone.'): RouteHealth {
  if (!input.link) return { state: 'unavailable', reason: notSetUp };
  if (!input.link.paired || !input.link.connected) return { state: 'needs_you', needsYou: needsYou('BT_NOT_PAIRED') };
  if (input.locked === true) return { state: 'unavailable', reason: 'The iPhone is locked; a keyboard and mouse need it unlocked.' };
  if (!input.broadcast) return { state: 'needs_you', needsYou: needsYou('BROADCAST_STOPPED') };
  if (input.pointerSeen === false) return { state: 'needs_you', needsYou: needsYou('ASSISTIVETOUCH_OFF') };
  return { state: 'healthy' };
}

// ── The core ────────────────────────────────────────────────────────────────

/** The pointer and keyboard method set every route sees (contract §6 HidPointer). */
export interface HidPointer {
  calibrate(): Promise<boolean>;
  point(x: number, y: number, aim?: boolean): Promise<void>;
  tap(x: number, y: number, aim?: boolean): Promise<void>;
  drag(points: Point[], hold?: boolean): Promise<void>;
  scroll(x: number, y: number, lines: number): Promise<void>;
  type(text: string): Promise<void>;
  key(combo: string): Promise<void>;
  goHome(): Promise<void>;
  goBack(): Promise<void>;
  reset(): Promise<void>;
  forget(): void;
  where(): Point | null;
  calibration(): Calibration | null;
}

export interface HidCoreDeps {
  sender: HidSender;
  screen: PointerScreen | null;
  load: () => Calibration | null;
  save: (c: Calibration) => void;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}

export class HidCore implements HidPointer {
  private cal: Calibration | null;
  private pos: Point | null = null;
  /** Last known pointer position as screen fractions (what the screen shows), for masking it out of comparisons. */
  private seenAt: Point | null = null;
  private moves = 0;
  private link = -1;
  private readonly sender: HidSender;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly now: () => number;

  constructor(private readonly deps: HidCoreDeps) {
    this.sender = deps.sender;
    this.cal = deps.load();
    this.sleep = deps.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
    this.now = deps.now ?? Date.now;
  }

  get senderId(): HidSenderId {
    return this.sender.id;
  }

  where(): Point | null {
    return this.seenAt;
  }

  get calibrated(): boolean {
    return this.cal !== null;
  }

  calibration(): Calibration | null {
    return this.cal ? structuredClone(this.cal) : null;
  }

  /** Every send goes through here: after any failure the pointer position is unknown. */
  private async io(run: () => Promise<void>): Promise<void> {
    try {
      await run();
    } catch (error) {
      this.pos = null;
      throw error;
    }
  }

  /** A relative move, split into steps the sender accepts. */
  private async nudge(dx: number, dy: number): Promise<void> {
    for (const [sx, sy] of splitMove(dx, dy, this.sender.maxStep)) await this.io(() => this.sender.pointerMove(sx, sy));
  }

  // ── calibration ───────────────────────────────────────────────────────────

  /** Finds the pointer at two distances from the corner and fits the mapping. False when it can't see the pointer. */
  async calibrate(): Promise<boolean> {
    this.pos = null;
    await this.io(() => this.sender.releaseAll());
    await this.io(() => this.sender.pointerReset());
    const seen: Array<[number, number, number]> = [];
    let at = 0;
    for (const units of CALIBRATION_STOPS) {
      const step = units - at;
      await this.nudge(step, step);
      at = units;
      const found = this.deps.screen?.live() ? await this.findAnywhere() : null;
      if (!found) break;
      seen.push([units, found[0], found[1]]);
    }
    if (seen.length < 2) return false;
    const [[u1, x1, y1], [u2, x2, y2]] = seen;
    if (x2 - x1 < 0.01 || y2 - y1 < 0.01) return false;
    const cal: Calibration = { w: Math.round((u2 - u1) / (x2 - x1)), h: Math.round((u2 - u1) / (y2 - y1)), seen: { x: [], y: [] } };
    if (cal.w < 50 || cal.h < 50 || cal.w > 20_000 || cal.h > 20_000) return false;
    for (const [units, x, y] of seen) {
      cal.seen.x.push([units / cal.w, x]);
      cal.seen.y.push([units / cal.h, y]);
    }
    this.cal = cal;
    this.deps.save(cal);
    this.pos = [CALIBRATION_STOPS[1], CALIBRATION_STOPS[1]];
    this.seenAt = [x2, y2];
    return true;
  }

  /** Wiggles the pointer and finds it anywhere on screen. */
  private async findAnywhere(): Promise<Point | null> {
    const screen = this.deps.screen;
    if (!screen) return null;
    await screen.settled(200, 300, 1_500);
    const before = screen.latestGray();
    const sent = this.now();
    await this.nudge(NUDGE, 0);
    let after = await screen.nextGray(sent, 1_000);
    if (after) {
      await screen.settled(150, 100, 600);
      after = screen.latestGray();
    }
    await this.nudge(-NUDGE, 0);
    if (!before || !after) return null;
    const found = findPointer(before, after, [0.5, 0.5], 0.025, [0.5, 0.5]);
    return found && found[0] > 0.03 && found[0] < 0.97 && found[1] > 0.03 && found[1] < 0.97 ? found : null;
  }

  // ── moves ─────────────────────────────────────────────────────────────────

  private fix(t: number, axis: 'x' | 'y'): number {
    const pts = this.cal?.seen[axis] ?? [];
    if (pts.length < 2) return t;
    const n = pts.length;
    const mc = pts.reduce((s, [c]) => s + c, 0) / n;
    const mf = pts.reduce((s, [, f]) => s + f, 0) / n;
    const v = pts.reduce((s, [c]) => s + (c - mc) ** 2, 0);
    let a = v > 1e-4 ? pts.reduce((s, [c, f]) => s + (c - mc) * (f - mf), 0) / v : 1;
    if (!(a > 0.85 && a < 1.15)) a = 1;
    const b = mf - a * mc;
    return Math.abs(b) < 0.1 ? (t - b) / a : t;
  }

  private units(t: number, axis: 'x' | 'y'): number {
    if (!this.cal) throw new Error('not calibrated');
    const span = axis === 'x' ? this.cal.w : this.cal.h;
    return Math.round(Math.min(Math.max(this.fix(t, axis), 0), 1) * span);
  }

  private async moveTo(tx: number, ty: number, rehome = true): Promise<void> {
    const gen = this.sender.linkGeneration();
    if (gen !== this.link) {
      // New link: a dropped gesture may have left the button down.
      await this.io(() => this.sender.releaseAll());
      this.pos = null;
      this.link = gen;
    }
    if (!this.pos || (rehome && this.moves >= REHOME_EVERY)) {
      await this.io(() => this.sender.pointerReset());
      this.pos = [0, 0];
      this.moves = 0;
    }
    const [x, y] = this.pos;
    this.pos = null;
    await this.nudge(tx - x, ty - y);
    this.pos = [tx, ty];
    this.moves += 1;
  }

  private requireCal(): Calibration {
    if (!this.cal) throw new HidError('not_calibrated', 'The pointer is not calibrated yet. Run Calibrate in Settings → Computers → Phones.');
    return this.cal;
  }

  async point(x: number, y: number, aim = false): Promise<void> {
    this.requireCal();
    await this.moveTo(this.units(x, 'x'), this.units(y, 'y'));
    this.seenAt = [x, y];
    if (aim && this.deps.screen?.live()) await this.aim(x, y);
  }

  /** Closed loop: find the real pointer near the target, fix a miss and learn from it. */
  private async aim(x: number, y: number): Promise<void> {
    const cal = this.requireCal();
    const screen = this.deps.screen;
    if (!screen) return;
    const nudge = x < 0.9 ? NUDGE : -NUDGE;
    await screen.settled(150, 100, 600);
    const before = screen.latestGray();
    const sent = this.now();
    await this.nudge(nudge, 0);
    let after = await screen.nextGray(sent, 600);
    if (after) {
      await screen.settled(100, 50, 400);
      after = screen.latestGray();
    }
    await this.nudge(-nudge, 0);
    if (!before || !after) return;
    const shift = nudge / cal.w;
    const found = findPointer(before, after, [x, y], shift) ?? findPointer(before, after, [0.5, 0.5], shift, [0.5, 0.5]);
    if (!found || !this.pos) return;
    const [fx, fy] = found;
    if (Math.max(Math.abs(fx - x), Math.abs(fy - y)) <= MAX_MISS) {
      cal.seen.x.push([this.fix(x, 'x'), fx]);
      cal.seen.y.push([this.fix(y, 'y'), fy]);
      cal.seen.x = cal.seen.x.slice(-MAX_SEEN);
      cal.seen.y = cal.seen.y.slice(-MAX_SEEN);
      this.deps.save(cal);
    }
    if (Math.abs(fx - x) <= AIM_TOLERANCE && Math.abs(fy - y) <= AIM_TOLERANCE) return;
    const dx = Math.round((x - fx) * cal.w);
    const dy = Math.round((y - fy) * cal.h);
    const [px, py] = this.pos;
    await this.nudge(dx, dy);
    this.pos = [px + dx, py + dy];
  }

  async tap(x: number, y: number, aim = true): Promise<void> {
    await this.point(x, y, aim);
    await this.io(() => this.sender.pointerButton('click'));
    await this.sleep(TAP_GAP_MS);
  }

  /** One continuous stroke. hold=true rests at the end so lists stop there instead of flinging on. */
  async drag(points: Point[], hold = true): Promise<void> {
    if (points.length < 2) throw new Error('a drag needs two points');
    await this.point(points[0][0], points[0][1]);
    await this.io(() => this.sender.pointerButton('press'));
    try {
      for (const [x, y] of points.slice(1)) {
        await this.moveTo(this.units(x, 'x'), this.units(y, 'y'), false);
        this.seenAt = [x, y];
      }
      if (hold) await this.sleep(HOLD_MS);
    } finally {
      await this.io(() => this.sender.pointerButton('release'));
    }
  }

  async scroll(x: number, y: number, lines: number): Promise<void> {
    await this.point(x, y);
    await this.io(() => this.sender.scroll(lines));
  }

  // ── keys ──────────────────────────────────────────────────────────────────

  /** Printable text on the phone's hardware-keyboard layout (US; see phone-hid-keys.ts for the limit). */
  async type(text: string): Promise<void> {
    const bad = untypable(text);
    if (bad !== null) throw new HidError('bad_params', `The keyboard can’t type “${bad}”: only printable ASCII on a US layout can be sent.`);
    await this.io(() => this.sender.typeText(text));
  }

  /** One combo, e.g. "enter" or "cmd+space". */
  async key(combo: string): Promise<void> {
    const stroke = parseCombo(combo);
    if (!stroke) throw new HidError('bad_params', `Unknown key combo: ${combo}.`);
    await this.io(() => this.sender.key(stroke));
  }

  /** Spotlight: ⌘Space, type the name, Return. */
  async openApp(name: string): Promise<void> {
    for (const step of spotlightOpenSteps(name)) {
      if (step.kind === 'combo') await this.key(step.combo);
      else if (step.kind === 'text') await this.type(step.text);
      else await this.sleep(step.ms);
    }
  }

  /** ⌘H: the Home Screen from the keyboard (no pointer needed). */
  async pressHome(): Promise<void> {
    await this.key(HOME_COMBO);
  }

  /** One Full Keyboard Access move (Tab, arrows, Space to activate, Escape to go back). */
  async fka(step: FkaStep): Promise<void> {
    await this.key(FKA_KEYS[step]);
  }

  // ── pointer gestures ──────────────────────────────────────────────────────

  async goHome(): Promise<void> {
    await this.drag(HOME_SWIPE, false); // resting would open the app switcher
  }

  /** The same swipe up from the bottom edge, resting at the end: the App Switcher. */
  async appSwitcher(): Promise<void> {
    await this.drag(HOME_SWIPE, true);
  }

  async goBack(): Promise<void> {
    await this.drag(BACK_SWIPE, false);
  }

  /** Lets go of the button and re-homes, whatever state a gesture left. */
  async reset(): Promise<void> {
    this.pos = null;
    await this.io(() => this.sender.releaseAll());
    await this.io(() => this.sender.pointerReset());
    this.pos = [0, 0];
    this.seenAt = [0, 0];
    this.moves = 0;
  }

  forget(): void {
    this.cal = null;
    this.pos = null;
  }

  // ── act, then compare against the screen ──────────────────────────────────

  /**
   * Runs one action and says whether the screen changed afterwards, with the
   * pointer's own trip (where it was, and every point `run` returns) masked
   * out. Null when there was no frame to compare against. The frame before is
   * read before `run` starts, in the same tick as the caller's own read.
   */
  async actAndCompare(run: () => Promise<Point[]>): Promise<boolean | null> {
    const screen = this.deps.screen;
    const beforeAt = screen?.latestAt() ?? null;
    const before = screen && beforeAt !== null ? screen.latestGray() : null;
    const from = this.where();
    const visited = await run();
    if (!screen || beforeAt === null) return null;
    await screen.settled(350, 300, 3_000);
    const afterAt = screen.latestAt();
    const after = afterAt !== null && afterAt !== beforeAt ? screen.latestGray() : before;
    return changed(before, after, [...(from ? [from] : []), ...visited]);
  }
}

/** Swipe up from the bottom edge (home indicator). */
export const HOME_SWIPE: Point[] = [
  [0.5, 0.995],
  [0.5, 0.6],
];
/** Swipe right from the left edge. */
export const BACK_SWIPE: Point[] = [
  [0.005, 0.45],
  [0.6, 0.45],
];
