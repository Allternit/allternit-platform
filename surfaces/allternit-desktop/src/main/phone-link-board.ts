/**
 * Allternit Link board lane: types only (Phase 1, lane 0). Behaviour lands in
 * lane C. Names and fields follow hardware/link/README.md sections 5.2, 5.3, 6.
 */

import type { FaceName } from './phone-usb-agent-protocol.js';
import type { ToolResult } from './phone-tools.js';

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
