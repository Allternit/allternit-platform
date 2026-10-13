/**
 * Allternit USB Agent wire protocol, USBA/1 (Desktop side).
 *
 * The board (hardware/usb-agent/firmware) is a USB mouse + keyboard plugged
 * into the phone. It dials Desktop over Wi-Fi and runs bounded input
 * commands. Everything security-relevant is here and pure, so it is tested
 * against a reference board with no hardware:
 *
 *   board → hello usba/1 <boardId> <fw> <resetReason> <bn>
 *   desk  → challenge <sn> <HMAC(K, "srv|id|bn|sn")>      Desktop proves it holds K
 *   board → auth <HMAC(K, "brd|id|bn|sn")>                the board proves it holds K
 *   desk  → ready
 *   desk  → c <seq> <command…> <tag>                      tag = HMAC(SK, "c|seq|command")[:32 hex]
 *   board → r <seq> ok | r <seq> err <reason>
 *   board → s armed=<0|1> usb=<mounted|asleep|none>       on change, unsolicited
 *
 * K is a 32-byte key per board, written over USB at setup (physical
 * presence). SK = HMAC(K, "ses|bn|sn") is fresh per connection, so a recorded
 * session can't be replayed into a new one; inside a session the board only
 * accepts strictly increasing sequence numbers.
 */

import { createHmac, randomBytes, timingSafeEqual } from 'node:crypto';

export const USBA_VERSION = 'usba/1';
export const USBA_DEFAULT_PORT = 7300;
export const USBA_MDNS_TYPE = '_allternit-usba._tcp';
/** Longest line either side accepts; anything longer drops the connection. */
export const USBA_MAX_LINE = 512;
/** The board releases everything and refuses input when it hears nothing authenticated for this long. */
export const USBA_LEASE_MS = 10_000;
export const USBA_HEARTBEAT_MS = 3_000;

const HEX = /^[0-9a-f]+$/;
const BOARD_ID = /^usba-[0-9a-f]{12}$/;

export function newKeyHex(): string {
  return randomBytes(32).toString('hex');
}

export function newNonceHex(): string {
  return randomBytes(16).toString('hex');
}

export function hmacHex(keyHex: string, message: string): string {
  return createHmac('sha256', Buffer.from(keyHex, 'hex')).update(message).digest('hex');
}

export function equalHex(a: string, b: string): boolean {
  if (a.length !== b.length || !HEX.test(a) || !HEX.test(b)) return false;
  return timingSafeEqual(Buffer.from(a, 'hex'), Buffer.from(b, 'hex'));
}

export const serverProof = (k: string, id: string, bn: string, sn: string) => hmacHex(k, `srv|${id}|${bn}|${sn}`);
export const boardProof = (k: string, id: string, bn: string, sn: string) => hmacHex(k, `brd|${id}|${bn}|${sn}`);
export const sessionKey = (k: string, bn: string, sn: string) => hmacHex(k, `ses|${bn}|${sn}`);
export const commandTag = (sk: string, seq: number, command: string) => hmacHex(sk, `c|${seq}|${command}`).slice(0, 32);

export interface Hello {
  boardId: string;
  firmware: string;
  resetReason: string;
  boardNonce: string;
}

export function parseHello(line: string): Hello | null {
  const parts = line.trim().split(' ');
  if (parts.length !== 6 || parts[0] !== 'hello' || parts[1] !== USBA_VERSION) return null;
  const [, , boardId, firmware, resetReason, boardNonce] = parts;
  if (!BOARD_ID.test(boardId) || !/^[0-9A-Za-z.\-]{1,24}$/.test(firmware) || !/^[a-z-]{1,16}$/.test(resetReason)) return null;
  if (boardNonce.length !== 32 || !HEX.test(boardNonce)) return null;
  return { boardId, firmware, resetReason, boardNonce };
}

export function parseAuth(line: string): string | null {
  const m = /^auth ([0-9a-f]{64})$/.exec(line.trim());
  return m ? m[1] : null;
}

export type UsbStatus = 'mounted' | 'asleep' | 'none';

export interface BoardState {
  armed: boolean;
  usb: UsbStatus;
}

export function parseState(line: string): BoardState | null {
  const m = /^s armed=([01]) usb=(mounted|asleep|none)$/.exec(line.trim());
  return m ? { armed: m[1] === '1', usb: m[2] as UsbStatus } : null;
}

export type Reply = { seq: number; ok: true } | { seq: number; ok: false; reason: string };

export function parseReply(line: string): Reply | null {
  const m = /^r (\d{1,10}) (ok|err(?: (.{1,200}))?)$/.exec(line.trim());
  if (!m) return null;
  const seq = Number(m[1]);
  return m[2] === 'ok' ? { seq, ok: true } : { seq, ok: false, reason: m[3] ?? 'error' };
}

// ── Commands ────────────────────────────────────────────────────────────────

export type BoardCommand =
  | { op: 'ping' }
  | { op: 'disarm' }
  | { op: 'home' }
  | { op: 'move'; dx: number; dy: number }
  | { op: 'press' }
  | { op: 'release' }
  | { op: 'click' }
  | { op: 'scroll'; v: number; h: number }
  | { op: 'type'; text: string }
  | { op: 'key'; combo: string };

export const MOVE_LIMIT = 4000;
export const TYPE_LIMIT = 200;
const KEY_NAMES = new Set(['enter', 'esc', 'tab', 'backspace', 'space', 'up', 'down', 'left', 'right', 'cmd', 'ctrl', 'alt', 'shift']);

export function isValidCombo(combo: string): boolean {
  const keys = combo.split('+');
  return keys.length >= 1 && keys.length <= 4 && keys.every((k) => KEY_NAMES.has(k) || /^[a-z0-9]$/.test(k));
}

/** Text form of a command, or null when it's out of bounds. Printable ASCII only: the board types with a US layout. */
export function encodeCommand(cmd: BoardCommand): string | null {
  const int = (n: number, limit: number) => Number.isInteger(n) && Math.abs(n) <= limit;
  switch (cmd.op) {
    case 'ping':
    case 'disarm':
    case 'home':
    case 'press':
    case 'release':
    case 'click':
      return cmd.op;
    case 'move':
      return int(cmd.dx, MOVE_LIMIT) && int(cmd.dy, MOVE_LIMIT) ? `move ${cmd.dx} ${cmd.dy}` : null;
    case 'scroll':
      return int(cmd.v, 50) && int(cmd.h, 50) ? `scroll ${cmd.v} ${cmd.h}` : null;
    case 'type':
      return cmd.text.length > 0 && cmd.text.length <= TYPE_LIMIT && /^[\x20-\x7E]*$/.test(cmd.text)
        ? `type ${Buffer.from(cmd.text, 'ascii').toString('hex')}`
        : null;
    case 'key':
      return isValidCombo(cmd.combo) ? `key ${cmd.combo}` : null;
  }
}

export function commandLine(sk: string, seq: number, command: string): string {
  return `c ${seq} ${command} ${commandTag(sk, seq, command)}`;
}

// ── Reference board (the firmware's logic, for tests and the simulator) ─────

/**
 * What the firmware does with each line, in TypeScript. The tests drive the
 * Desktop link against this, and hardware/usb-agent/firmware mirrors it line
 * for line, so a change here without the firmware change fails review.
 */
export class ReferenceBoard {
  armed = false;
  usb: UsbStatus = 'mounted';
  readonly log: string[] = [];
  private bn = newNonceHex();
  private sk: string | null = null;
  private lastSeq = 0;

  constructor(readonly boardId: string, private readonly keyHex: string, readonly firmware = '1.0.0') {}

  hello(): string {
    this.bn = newNonceHex();
    this.sk = null;
    this.lastSeq = 0;
    return `hello ${USBA_VERSION} ${this.boardId} ${this.firmware} power-on ${this.bn}`;
  }

  /** Returns the reply line(s), or 'drop' to close the connection. */
  receive(line: string): string[] | 'drop' {
    const trimmed = line.trim();
    if (!this.sk) {
      const m = /^challenge ([0-9a-f]{32}) ([0-9a-f]{64})$/.exec(trimmed);
      if (!m) return 'drop';
      const [, sn, proof] = m;
      if (!equalHex(proof, serverProof(this.keyHex, this.boardId, this.bn, sn))) return 'drop';
      this.sk = sessionKey(this.keyHex, this.bn, sn);
      return [`auth ${boardProof(this.keyHex, this.boardId, this.bn, sn)}`];
    }
    if (trimmed === 'ready') return [`s armed=${this.armed ? 1 : 0} usb=${this.usb}`];
    const m = /^c (\d{1,10}) (.+) ([0-9a-f]{32})$/.exec(trimmed);
    if (!m) return 'drop';
    const seq = Number(m[1]);
    const command = m[2];
    if (!equalHex(m[3], commandTag(this.sk, seq, command))) return 'drop';
    if (seq <= this.lastSeq) return [`r ${seq} err replay`];
    this.lastSeq = seq;
    const op = command.split(' ')[0];
    if (op === 'ping') return [`r ${seq} ok`];
    if (op === 'disarm') {
      this.armed = false;
      return [`r ${seq} ok`, 's armed=0 usb=' + this.usb];
    }
    if (!this.armed) return [`r ${seq} err disarmed`];
    if (this.usb !== 'mounted') return [`r ${seq} err phone usb asleep`];
    this.log.push(command);
    return [`r ${seq} ok`];
  }
}
