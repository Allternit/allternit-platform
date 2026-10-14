/**
 * Touch-like gestures on an iPhone, built from relative mouse moves through
 * the USB Agent (iOS AssistiveTouch turns the mouse into an on-screen
 * pointer). Ported from esp32-iphone-agent's phone.py (MIT, Henry Yuan; see
 * hardware/usb-agent/NOTICE).
 *
 * The pointer is relative, so we track where it is in mouse units and
 * re-home it (slam it into the top-left corner) every few gestures to cancel
 * drift. Calibration finds the pointer on the mirrored screen at two known
 * distances from the corner; after that, every aimed tap checks where the
 * pointer really is, fixes a miss and learns from it. Coordinates are
 * fractions of the screen (0..1, origin top-left).
 */

import type { SendResult } from './phone-usb-agent-link.js';
import type { BoardCommand } from './phone-usb-agent-protocol.js';
import { findPointer, type Gray } from './phone-usb-agent-vision.js';

export const CALIBRATION_STOPS = [150, 350] as const;
export const REHOME_EVERY = 10;
export const NUDGE = 12;
export const AIM_TOLERANCE = 0.008;
export const MAX_MISS = 0.06;
const HOLD_MS = 250;
const TAP_GAP_MS = 150;
const MAX_SEEN = 12;

export interface Calibration {
  /** Mouse units to cross the full screen width / height. */
  w: number;
  h: number;
  seen: { x: Array<[number, number]>; y: Array<[number, number]> };
}

export interface PointerScreen {
  /** Mirroring right now. */
  live(): boolean;
  latestAt(): number | null;
  /** Grayscale of the newest frame. */
  latestGray(): Gray | null;
  /** Grayscale of the first frame after `since`, or null. */
  nextGray(since: number, timeoutMs: number): Promise<Gray | null>;
  settled(quietMs: number, leastMs: number, mostMs: number): Promise<void>;
}

export interface PointerDeps {
  send: (cmd: BoardCommand) => Promise<SendResult>;
  screen: PointerScreen | null;
  load: () => Calibration | null;
  save: (c: Calibration) => void;
  /** Bumps whenever the board reconnects; a new link may have dropped a gesture mid-way. */
  linkGeneration: () => number;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}

/** A board command failed; carries the link's result so tools report it as-is. */
export class BoardError extends Error {
  constructor(readonly result: Extract<SendResult, { ok: false }>) {
    super(result.message);
  }
}

export class UsbPointer {
  private cal: Calibration | null;
  private pos: [number, number] | null = null;
  /** Last known pointer position as screen fractions (what the screen shows), for masking it out of comparisons. */
  private seenAt: [number, number] | null = null;
  private moves = 0;
  private link = -1;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly now: () => number;

  constructor(private readonly deps: PointerDeps) {
    this.cal = deps.load();
    this.sleep = deps.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
    this.now = deps.now ?? Date.now;
  }

  where(): [number, number] | null {
    return this.seenAt;
  }

  get calibrated(): boolean {
    return this.cal !== null;
  }

  calibration(): Calibration | null {
    return this.cal ? structuredClone(this.cal) : null;
  }

  private async send(cmd: BoardCommand): Promise<void> {
    const res = await this.deps.send(cmd);
    if (!res.ok) {
      this.pos = null; // after any failure the pointer position is unknown
      throw new BoardError(res);
    }
  }

  // ── calibration ───────────────────────────────────────────────────────────

  /** Finds the pointer at two distances from the corner and fits the mapping. False when it can't see the pointer. */
  async calibrate(): Promise<boolean> {
    this.pos = null;
    await this.send({ op: 'release' });
    await this.send({ op: 'home' });
    const seen: Array<[number, number, number]> = [];
    let at = 0;
    for (const units of CALIBRATION_STOPS) {
      const step = units - at;
      await this.send({ op: 'move', dx: step, dy: step });
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
  private async findAnywhere(): Promise<[number, number] | null> {
    const screen = this.deps.screen;
    if (!screen) return null;
    await screen.settled(200, 300, 1_500);
    const before = screen.latestGray();
    const sent = this.now();
    await this.send({ op: 'move', dx: NUDGE, dy: 0 });
    let after = await screen.nextGray(sent, 1_000);
    if (after) {
      await screen.settled(150, 100, 600);
      after = screen.latestGray();
    }
    await this.send({ op: 'move', dx: -NUDGE, dy: 0 });
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
    const gen = this.deps.linkGeneration();
    if (gen !== this.link) {
      // New link: a dropped gesture may have left the button down.
      await this.send({ op: 'release' });
      this.pos = null;
      this.link = gen;
    }
    if (!this.pos || (rehome && this.moves >= REHOME_EVERY)) {
      await this.send({ op: 'home' });
      this.pos = [0, 0];
      this.moves = 0;
    }
    const [x, y] = this.pos;
    this.pos = null;
    await this.send({ op: 'move', dx: tx - x, dy: ty - y });
    this.pos = [tx, ty];
    this.moves += 1;
  }

  private requireCal(): Calibration {
    if (!this.cal) throw new BoardError({ ok: false, error: 'rejected', message: 'The pointer is not calibrated yet. Run Calibrate in Settings → Computers → Phones.' });
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
    await this.send({ op: 'move', dx: nudge, dy: 0 });
    let after = await screen.nextGray(sent, 600);
    if (after) {
      await screen.settled(100, 50, 400);
      after = screen.latestGray();
    }
    await this.send({ op: 'move', dx: -nudge, dy: 0 });
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
    await this.send({ op: 'move', dx, dy });
    this.pos = [px + dx, py + dy];
  }

  async tap(x: number, y: number, aim = true): Promise<void> {
    await this.point(x, y, aim);
    await this.send({ op: 'click' });
    await this.sleep(TAP_GAP_MS);
  }

  /** One continuous stroke. hold=true rests at the end so lists stop there instead of flinging on. */
  async drag(points: Array<[number, number]>, hold = true): Promise<void> {
    if (points.length < 2) throw new Error('a drag needs two points');
    await this.point(points[0][0], points[0][1]);
    await this.send({ op: 'press' });
    try {
      for (const [x, y] of points.slice(1)) {
        await this.moveTo(this.units(x, 'x'), this.units(y, 'y'), false);
        this.seenAt = [x, y];
      }
      if (hold) await this.sleep(HOLD_MS);
    } finally {
      await this.send({ op: 'release' });
    }
  }

  async scroll(x: number, y: number, lines: number): Promise<void> {
    await this.point(x, y);
    await this.send({ op: 'scroll', v: lines, h: 0 });
  }

  async type(text: string): Promise<void> {
    await this.send({ op: 'type', text });
  }

  async key(combo: string): Promise<void> {
    await this.send({ op: 'key', combo });
  }

  async goHome(): Promise<void> {
    await this.drag([[0.5, 0.995], [0.5, 0.6]], false); // resting would open the app switcher
  }

  async goBack(): Promise<void> {
    await this.drag([[0.005, 0.45], [0.6, 0.45]], false);
  }

  /** Lets go of the button and re-homes, whatever state a gesture left. */
  async reset(): Promise<void> {
    this.pos = null;
    await this.send({ op: 'release' });
    await this.send({ op: 'home' });
    this.pos = [0, 0];
    this.seenAt = [0, 0];
    this.moves = 0;
  }

  forget(): void {
    this.cal = null;
    this.pos = null;
  }
}
