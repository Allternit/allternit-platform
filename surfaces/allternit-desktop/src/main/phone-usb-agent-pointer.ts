/**
 * The USB Agent board as a keyboard/mouse sender (`usb-agent`, contract §6).
 * The board is a USB mouse and keyboard in the iPhone's port; iOS
 * AssistiveTouch turns the mouse into an on-screen pointer. Aiming, gestures
 * and keys live in the shared core (phone-hid-core.ts, ported from
 * esp32-iphone-agent's phone.py, MIT, Henry Yuan; see hardware/usb-agent/NOTICE);
 * this file only turns the core's sends into USBA/1 board commands.
 */

import type { SendResult } from './phone-usb-agent-link.js';
import { MOVE_LIMIT, type BoardCommand } from './phone-usb-agent-protocol.js';
import { HidCore, type Calibration, type HidLinkStatus, type HidSender, type PointerScreen } from './phone-hid-core.js';

export { AIM_TOLERANCE, CALIBRATION_STOPS, MAX_MISS, NUDGE, REHOME_EVERY } from './phone-hid-core.js';
export type { Calibration, PointerScreen } from './phone-hid-core.js';

export interface PointerDeps {
  send: (cmd: BoardCommand) => Promise<SendResult>;
  screen: PointerScreen | null;
  load: () => Calibration | null;
  save: (c: Calibration) => void;
  /** Bumps whenever the board reconnects; a new link may have dropped a gesture mid-way. */
  linkGeneration: () => number;
  /** The board's Wi-Fi link is up. A cable needs no pairing, so `paired` is always true. Defaults to connected. */
  connected?: () => boolean;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}

/** A board command failed; carries the link's result so tools report it as-is. */
export class BoardError extends Error {
  constructor(readonly result: Extract<SendResult, { ok: false }>) {
    super(result.message);
  }
}

/** The board as a HidSender: one USBA/1 command per send. Failures throw BoardError. */
export function boardSender(deps: Pick<PointerDeps, 'send' | 'linkGeneration' | 'connected'>): HidSender {
  const run = async (cmd: BoardCommand): Promise<void> => {
    const res = await deps.send(cmd);
    if (!res.ok) throw new BoardError(res);
  };
  return {
    id: 'usb-agent',
    maxStep: MOVE_LIMIT,
    linkGeneration: deps.linkGeneration,
    status: async (): Promise<HidLinkStatus> => ({ paired: true, connected: deps.connected?.() ?? true, name: 'USB Agent' }),
    pointerMove: (dx, dy) => run({ op: 'move', dx, dy }),
    pointerButton: (action) => run({ op: action }),
    pointerReset: () => run({ op: 'home' }),
    scroll: (lines) => run({ op: 'scroll', v: lines, h: 0 }),
    // The board takes the combo text and presses it itself.
    key: (stroke) => run({ op: 'key', combo: stroke.combo }),
    typeText: (text) => run({ op: 'type', text }),
    releaseAll: () => run({ op: 'release' }),
  };
}

/** The shared core driving the board: the pointer the USB Agent manager has always used. */
export class UsbPointer extends HidCore {
  constructor(deps: PointerDeps) {
    super({ sender: boardSender(deps), screen: deps.screen, load: deps.load, save: deps.save, sleep: deps.sleep, now: deps.now });
  }
}
