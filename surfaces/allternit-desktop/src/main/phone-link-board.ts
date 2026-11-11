/**
 * Allternit Link composite lane: a board's hands, an app iPhone's eyes
 * (hardware/link/README.md sections 5 and 6).
 *
 * An iPhone connected with the Allternit app shares its screen with the node;
 * a USB Agent / Link board in its port is its mouse and keyboard. Pairing the
 * two (StoredUsbAgent.pairedPhone) makes one phone: the app serial `app:<id>`
 * takes taps. The board's pointer, calibration and safety blockers are the
 * USB Agent's own (UsbAgentManager); only the frame source differs, and every
 * tap still crosses PhoneToolRunner.dispatch, so the approval guard applies.
 *
 * This file routes the hands tools, remembers that a board was disarmed so a
 * run stops at once, and sends the board faces (best effort, USBA/2 only).
 */

import type { FaceName } from './phone-usb-agent-protocol.js';
import type { ToolResult } from './phone-tools.js';
import type { UsbAgentManager } from './phone-usb-agent.js';

export interface LinkFrame {
  jpeg: Buffer;
  width: number;
  height: number;
  at: number;
}

/** The slice of a screen the board lane needs. AirplayScreen already has this shape. */
export interface LinkFrameSource {
  state(): 'unavailable' | 'stopped' | 'waiting' | 'mirroring';
  latest(): LinkFrame | null;
  nextFrame(since: number, timeoutMs: number): Promise<LinkFrame | null>;
  settled(quietMs: number, leastMs: number, mostMs: number): Promise<void>;
  start(): void;
  stop(): void;
  /**
   * Optional: join the picture now and wait up to timeoutMs for the first
   * frame. The app stream is joined on demand; AirPlay has no such step.
   */
  wake?(timeoutMs: number): Promise<void>;
}

export interface LinkBoardHands {
  /** The board paired to this app phone, or null. */
  boardFor(serial: string): string | null;
  /** Runs a hands tool for a paired app phone. */
  call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
  status(serial: string): LinkBoardStatus | null;
}

export type LinkBoardState =
  | 'board_offline'
  | 'phone_not_connected'
  | 'phone_locked'
  | 'no_screen'
  | 'not_calibrated'
  | 'disarmed'
  | 'ready';

export interface LinkBoardStatus {
  boardId: string; // "usba-0123456789ab"
  serial: string; // "usb:<boardId>"
  name: string;
  online: boolean; // a session with Desktop exists
  armed: boolean;
  status: LinkBoardState;
  link: 'lan' | 'cloud' | null; // null when offline or USBA/1
  usb: 'mounted' | 'asleep' | 'none' | null;
  hid: 'usb' | 'ble' | 'none' | null;
  dock: boolean | null;
  face: FaceName | null;
  prog: number | null; // 0 to 5
  eyes: 'airplay' | 'app';
  phone: string | null; // paired app phone serial, "app:<uuid>"
  screen: boolean; // the eyes have a live picture
  calibrated: boolean;
  firmware: string | null;
  protocol: 1 | 2 | null;
  firmwareUpdate: boolean;
  relay: 'registered' | 'none';
}

/** Every tool UsbAgentManager.call answers, including the ones it answers as unsupported. */
const HANDS_TOOLS: ReadonlySet<string> = new Set([
  'phone.screenshot',
  'phone.tap',
  'phone.swipe',
  'phone.type',
  'phone.home',
  'phone.back',
  'phone.key',
  'phone.open_app',
  'phone.task',
  'phone.read_ui',
  'phone.screen_text',
  'phone.find_and_tap',
  'phone.sms.send',
  'phone.call.dial',
]);

export function isHandsTool(name: string): boolean {
  return HANDS_TOOLS.has(name);
}

/** What a paired app phone's note says while taps don't work, by board status. */
export const LINK_STATUS_TEXT: Record<LinkBoardState, string> = {
  board_offline: 'the Link board is not connected',
  phone_not_connected: 'the Link board is not plugged into this iPhone',
  phone_locked: 'the iPhone is locked',
  no_screen: 'the screen is not shared',
  not_calibrated: 'the pointer is not calibrated yet (Settings → Computers → Phones)',
  disarmed: 'the Link board is disarmed; press its button to arm it',
  ready: 'ready',
};

/** After this long without a hands call, the board face goes back to idle. */
export const LINK_IDLE_MS = 30_000;
/** How long `wink` shows after a run ends well. */
export const LINK_WINK_MS = 1_000;
const HOLD_POLL_MS = 2_000;

/** The hold view the face follows (phone-hold-client.ts HoldView, only the field used). */
export type HoldStateLookup = (approvalId: string) => Promise<{ state: string } | null>;

const fail = (error: 'device_offline' | 'disarmed', message: string): ToolResult => ({ ok: false, error, message });

export class LinkBoardLane implements LinkBoardHands {
  private attached: UsbAgentManager | null = null;
  /** Boards that went from armed to disarmed: hands calls answer `disarmed` without sending until they arm again. */
  private readonly stopped = new Set<string>();
  /** Acting hands calls since the face was last idle, per board (the progress squares). */
  private readonly steps = new Map<string, number>();
  private readonly timers = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly idleMs: number;

  constructor(private readonly deps: { usb: () => UsbAgentManager | null; now?: () => number; idleMs?: number }) {
    this.idleMs = deps.idleMs ?? LINK_IDLE_MS;
  }

  /** The USB manager, watching its boards' armed state from the first time it is seen. */
  private manager(): UsbAgentManager | null {
    const m = this.deps.usb();
    if (m && m !== this.attached) {
      this.attached = m;
      this.stopped.clear();
      m.on('armed', (e: { boardId: string; armed: boolean }) => {
        if (this.attached !== m) return;
        if (e.armed) this.stopped.delete(e.boardId);
        else if (m.pairedPhone(e.boardId)) this.stopped.add(e.boardId);
      });
    }
    return m;
  }

  boardFor(serial: string): string | null {
    return this.manager()?.boardForPhone(serial) ?? null;
  }

  status(serial: string): LinkBoardStatus | null {
    const m = this.manager();
    const boardId = m?.boardForPhone(serial) ?? null;
    return boardId ? m?.status(boardId) ?? null : null;
  }

  async call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult> {
    const m = this.manager();
    const boardId = m?.boardForPhone(serial) ?? null;
    if (!m || !boardId) return fail('device_offline', `No Allternit Link board is paired with ${serial}.`);
    if (this.stopped.has(boardId)) {
      return fail('disarmed', 'The Allternit Link board was disarmed, so the bot stopped. Press the board’s button to let the bot use the phone again.');
    }
    const acting = name !== 'phone.screenshot';
    if (acting) {
      const n = Math.min(5, (this.steps.get(boardId) ?? 0) + 1);
      this.steps.set(boardId, n);
      void m.display(boardId, [{ op: 'face', name: 'working' }, { op: 'prog', n }]);
    }
    try {
      return await m.call(name, `usb:${boardId}`, args);
    } finally {
      if (acting) this.later(boardId, this.idleMs, () => this.idle(boardId));
    }
  }

  /** A run on a phone ended (phone.task returned). A good end winks, then goes idle. */
  runEnded(serial: string, ok: boolean): void {
    const m = this.manager();
    const boardId = m?.boardForPhone(serial) ?? null;
    if (!m || !boardId) return;
    this.steps.delete(boardId);
    if (!ok) return this.idle(boardId);
    void m.display(boardId, [{ op: 'face', name: 'wink' }, { op: 'prog', n: 0 }]);
    this.later(boardId, LINK_WINK_MS, () => this.idle(boardId));
  }

  /** A hold was made: the board shows `needs_you` until the hold is answered or expires. */
  noteHold(notice: { approvalId: string; serial: string; expiresAt: string }, lookup: HoldStateLookup, pollMs = HOLD_POLL_MS): void {
    const m = this.manager();
    const boardId = m?.boardForPhone(notice.serial) ?? null;
    if (!m || !boardId) return;
    const timer = this.timers.get(boardId);
    if (timer) clearTimeout(timer);
    this.timers.delete(boardId);
    void m.display(boardId, [{ op: 'face', name: 'needs_you' }]);
    const now = this.deps.now ?? Date.now;
    const until = Date.parse(notice.expiresAt);
    const poll = setInterval(() => {
      const expired = Number.isFinite(until) && now() >= until;
      void (expired ? Promise.resolve({ state: 'expired' }) : lookup(notice.approvalId).catch(() => null)).then((hold) => {
        if (!hold || hold.state === 'requested') return;
        clearInterval(poll);
        if (this.manager() === m && m.boardForPhone(notice.serial) === boardId) this.idle(boardId);
      });
    }, pollMs);
    poll.unref?.();
  }

  private idle(boardId: string): void {
    this.steps.delete(boardId);
    const timer = this.timers.get(boardId);
    if (timer) clearTimeout(timer);
    this.timers.delete(boardId);
    void this.attached?.display(boardId, [{ op: 'face', name: 'default' }, { op: 'prog', n: 0 }]);
  }

  private later(boardId: string, ms: number, run: () => void): void {
    const old = this.timers.get(boardId);
    if (old) clearTimeout(old);
    const timer = setTimeout(() => {
      this.timers.delete(boardId);
      run();
    }, ms);
    timer.unref?.();
    this.timers.set(boardId, timer);
  }

  dispose(): void {
    for (const t of this.timers.values()) clearTimeout(t);
    this.timers.clear();
  }
}
