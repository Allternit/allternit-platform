/**
 * Allternit USB Agent wire protocol, USBA/1 and USBA/2 (Desktop side).
 * Contract: hardware/link/README.md section 2.
 *
 * The board (hardware/usb-agent/firmware) is a USB mouse + keyboard plugged
 * into the phone. It dials Desktop over Wi-Fi (or through the cloud relay on
 * USBA/2) and runs bounded input commands. Everything security-relevant is
 * here and pure, so it is tested against a reference board with no hardware:
 *
 *   board → hello usba/<v> <boardId> <fw> <resetReason> <bn>
 *   desk  → challenge <sn> <HMAC(K, "srv|id|bn|sn")>      v2 label: srv2
 *   board → auth <HMAC(K, "brd|id|bn|sn")>                v2 label: brd2
 *   desk  → ready
 *   desk  → c <seq> <command…> <tag>                      tag = HMAC(SK, "c|seq|command")[:32 hex]
 *   board → v1: r <seq> ok | r <seq> err <reason>         (unsigned)
 *           v2: r <seq> <result> <HMAC(SK, "r|seq|result")[:32 hex]>
 *   board → v1: s armed=<0|1> usb=<mounted|asleep|none>   (unsigned)
 *           v2: s armed=… usb=… dock=… hid=… link=… face=… prog=… n=<count> <HMAC(SK, "s|<fields> n=<count>")[:32 hex]>
 *
 * K is a 32-byte key per board, written over USB at setup (physical
 * presence). SK = HMAC(K, "ses|bn|sn") (v2: "ses2") is fresh per connection,
 * so a recorded session can't be replayed into a new one; inside a session
 * the board only accepts strictly increasing sequence numbers. The version is
 * bound into every v2 label, so a relay that rewrites usba/2 to usba/1 breaks
 * the handshake. On v2, typed text goes as `typex` under EK = HMAC(K,
 * "enc2|bn|sn") with AES-256-GCM, so the relay cannot read it.
 */

import { createCipheriv, createDecipheriv, createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
import { FakeDisplay, renderFace } from './phone-usb-agent-face.js';

/** The USBA/1 version string. A board built for USBA/2 says `usba/2` (see parseHello). */
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

export type UsbaVersion = 1 | 2;

/** Handshake labels: USBA/2 binds the version into each one (`srv2`, `brd2`, `ses2`). */
const label = (base: string, v: UsbaVersion) => (v === 2 ? `${base}2` : base);

export const serverProof = (k: string, id: string, bn: string, sn: string, v: UsbaVersion = 1) =>
  hmacHex(k, `${label('srv', v)}|${id}|${bn}|${sn}`);
export const boardProof = (k: string, id: string, bn: string, sn: string, v: UsbaVersion = 1) =>
  hmacHex(k, `${label('brd', v)}|${id}|${bn}|${sn}`);
export const sessionKey = (k: string, bn: string, sn: string, v: UsbaVersion = 1) => hmacHex(k, `${label('ses', v)}|${bn}|${sn}`);
/** EK: the USBA/2 key for `typex` text. */
export const textKey = (k: string, bn: string, sn: string) => hmacHex(k, `enc2|${bn}|${sn}`);
export const commandTag = (sk: string, seq: number, command: string) => hmacHex(sk, `c|${seq}|${command}`).slice(0, 32);
/** USBA/2 reply tag; `result` is "ok" or "err <reason>". */
export const replyTag = (sk: string, seq: number, result: string) => hmacHex(sk, `r|${seq}|${result}`).slice(0, 32);
/** USBA/2 state tag; `body` is "<fields> n=<count>", the line without "s " and the tag. */
export const stateTag = (sk: string, body: string) => hmacHex(sk, `s|${body}`).slice(0, 32);

export interface Hello {
  version: UsbaVersion;
  boardId: string;
  firmware: string;
  resetReason: string;
  boardNonce: string;
}

export function parseHello(line: string): Hello | null {
  const parts = line.trim().split(' ');
  if (parts.length !== 6 || parts[0] !== 'hello') return null;
  const version: UsbaVersion | null = parts[1] === 'usba/1' ? 1 : parts[1] === 'usba/2' ? 2 : null;
  if (version === null) return null;
  const [, , boardId, firmware, resetReason, boardNonce] = parts;
  if (!BOARD_ID.test(boardId) || !/^[0-9A-Za-z.\-]{1,24}$/.test(firmware) || !/^[a-z-]{1,16}$/.test(resetReason)) return null;
  if (boardNonce.length !== 32 || !HEX.test(boardNonce)) return null;
  return { version, boardId, firmware, resetReason, boardNonce };
}

export function parseAuth(line: string): string | null {
  const m = /^auth ([0-9a-f]{64})$/.exec(line.trim());
  return m ? m[1] : null;
}

export type UsbStatus = 'mounted' | 'asleep' | 'none';

export type FaceName = 'default' | 'wink' | 'working' | 'needs_you' | 'stopped';
export const FACE_NAMES: readonly FaceName[] = ['default', 'wink', 'working', 'needs_you', 'stopped'];
export type HidTransport = 'usb' | 'ble' | 'none';
export type LinkKind = 'lan' | 'cloud';

export interface BoardState {
  armed: boolean;
  usb: UsbStatus;
  // Present on USBA/2 sessions only.
  dock?: boolean;
  hid?: HidTransport;
  link?: LinkKind;
  face?: FaceName;
  prog?: number;
}

/** The USBA/2 state fields in their fixed order (contract section 2.5), without `n` and the tag. */
export function stateFields(s: Required<BoardState>): string {
  return `armed=${s.armed ? 1 : 0} usb=${s.usb} dock=${s.dock ? 1 : 0} hid=${s.hid} link=${s.link} face=${s.face} prog=${s.prog}`;
}

const STATE_V2 =
  /^s (armed=([01]) usb=(mounted|asleep|none) dock=([01]) hid=(usb|ble|none) link=(lan|cloud) face=(default|wink|working|needs_you|stopped) prog=([0-5])(?: [a-z0-9_]{1,16}=[!-~]{1,64})* n=(\d{1,10})) ([0-9a-f]{32})$/;

/**
 * v1: the unsigned USBA/1 line. v2: the signed line; null when `sk` is
 * missing, the tag is wrong, or any field is malformed. Unknown `key=value`
 * tokens between `prog=` and `n=` are covered by the tag and ignored.
 */
export function parseState(line: string, v: UsbaVersion = 1, sk?: string): (BoardState & { n?: number }) | null {
  const t = line.trim();
  if (v === 1) {
    const m = /^s armed=([01]) usb=(mounted|asleep|none)$/.exec(t);
    return m ? { armed: m[1] === '1', usb: m[2] as UsbStatus } : null;
  }
  const m = STATE_V2.exec(t);
  if (!m || !sk) return null;
  const n = Number(m[9]);
  if (!Number.isSafeInteger(n) || n < 1 || !equalHex(m[10], stateTag(sk, m[1]))) return null;
  return {
    armed: m[2] === '1',
    usb: m[3] as UsbStatus,
    dock: m[4] === '1',
    hid: m[5] as HidTransport,
    link: m[6] as LinkKind,
    face: m[7] as FaceName,
    prog: Number(m[8]),
    n,
  };
}

export type Reply = { seq: number; ok: true } | { seq: number; ok: false; reason: string };

/** v1: the unsigned USBA/1 reply. v2: `r <seq> <result> <tag>`, null without `sk` or with a wrong tag. */
export function parseReply(line: string, v: UsbaVersion = 1, sk?: string): Reply | null {
  const t = line.trim();
  let seqText: string;
  let result: string;
  if (v === 1) {
    const m = /^r (\d{1,10}) (ok|err(?: (.{1,200}))?)$/.exec(t);
    if (!m) return null;
    seqText = m[1];
    result = m[2] === 'ok' ? 'ok' : `err ${m[3] ?? 'error'}`;
  } else {
    const m = /^r (\d{1,10}) (ok|err .{1,200}) ([0-9a-f]{32})$/.exec(t);
    if (!m || !sk) return null;
    // The tag covers the sequence number exactly as written.
    if (!equalHex(m[3], hmacHex(sk, `r|${m[1]}|${m[2]}`).slice(0, 32))) return null;
    seqText = m[1];
    result = m[2];
  }
  const seq = Number(seqText);
  return result === 'ok' ? { seq, ok: true } : { seq, ok: false, reason: result.slice(4) };
}

// ── typex: AES-256-GCM text under EK (contract section 2.7) ────────────────

function typexIv(seq: number): Buffer {
  const iv = Buffer.alloc(12);
  iv.writeBigUInt64BE(BigInt(seq), 4);
  return iv;
}

const isTypeText = (text: string) => text.length > 0 && text.length <= TYPE_LIMIT && /^[\x20-\x7E]*$/.test(text);

/** The sealed form of `type`: "typex <hex(ciphertext || 16-byte tag)>". Throws on text `type` would refuse. */
export function sealType(ek: string, seq: number, text: string): string {
  if (!isTypeText(text)) throw new RangeError('typex text must be 1 to 200 printable ASCII characters');
  if (!Number.isSafeInteger(seq) || seq < 0) throw new RangeError('typex needs a sequence number');
  const cipher = createCipheriv('aes-256-gcm', Buffer.from(ek, 'hex'), typexIv(seq));
  cipher.setAAD(Buffer.from(`c|${seq}|typex`, 'ascii'));
  const ct = Buffer.concat([cipher.update(Buffer.from(text, 'ascii')), cipher.final(), cipher.getAuthTag()]);
  return `typex ${ct.toString('hex')}`;
}

/** The board's half: the typed text, or null when GCM fails or the text is not what `type` takes. */
export function openTypex(ek: string, seq: number, hex: string): string | null {
  if (hex.length < 34 || hex.length > 432 || hex.length % 2 || !HEX.test(hex)) return null;
  const raw = Buffer.from(hex, 'hex');
  try {
    const decipher = createDecipheriv('aes-256-gcm', Buffer.from(ek, 'hex'), typexIv(seq));
    decipher.setAAD(Buffer.from(`c|${seq}|typex`, 'ascii'));
    decipher.setAuthTag(raw.subarray(raw.length - 16));
    const text = Buffer.concat([decipher.update(raw.subarray(0, raw.length - 16)), decipher.final()]).toString('latin1');
    return isTypeText(text) ? text : null;
  } catch {
    return null;
  }
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
  | { op: 'key'; combo: string }
  | { op: 'face'; name: FaceName }
  | { op: 'prog'; n: number }
  | { op: 'label'; text: string }; // '' clears it

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
    case 'face':
      return FACE_NAMES.includes(cmd.name) ? `face ${cmd.name}` : null;
    case 'prog':
      return Number.isInteger(cmd.n) && cmd.n >= 0 && cmd.n <= 5 ? `prog ${cmd.n}` : null;
    case 'label':
      if (cmd.text === '') return 'label -';
      return cmd.text.length <= 24 && /^[\x20-\x7E]+$/.test(cmd.text)
        ? `label ${Buffer.from(cmd.text, 'ascii').toString('hex')}`
        : null;
  }
}

export function commandLine(sk: string, seq: number, command: string): string {
  return `c ${seq} ${command} ${commandTag(sk, seq, command)}`;
}

// ── Reference board (the firmware's logic, for tests and the simulator) ─────

/** Input commands the firmware runs once armed; anything else answers `err unknown command`. */
const INPUT_OPS = new Set(['home', 'move', 'press', 'release', 'click', 'scroll', 'type', 'key']);
/** Partial refreshes before the next refresh is forced full (contract section 3.5). */
export const FULL_REFRESH_EVERY = 20;

/**
 * What the firmware does with each line, in TypeScript. The tests drive the
 * Desktop link against this, and hardware/usb-agent/firmware mirrors it line
 * for line, so a change here without the firmware change fails review.
 *
 * Version 2 (the default) adds signed replies and state lines, the display
 * commands `face`, `prog` and `label` (allowed while disarmed: they move
 * nothing), `typex`, and the faces the firmware sets itself. Version 1 keeps
 * the USBA/1 behaviour for boards that still run firmware 1.0.0.
 */
export class ReferenceBoard {
  armed = false;
  usb: UsbStatus = 'mounted';
  dock = false;
  link: LinkKind = 'lan';
  face: FaceName = 'default';
  prog = 0;
  label = '';
  readonly display = new FakeDisplay();
  /** Input commands the board ran, as plain text (`typex` is logged as the `type` it opened to). */
  readonly log: string[] = [];
  private bn = newNonceHex();
  private sk: string | null = null;
  private ek: string | null = null;
  private lastSeq = 0;
  private n = 0;
  private lastSentFields = '';
  private shown = '';
  private partials = 0;

  constructor(
    readonly boardId: string,
    private readonly keyHex: string,
    readonly firmware = '1.1.0',
    readonly version: UsbaVersion = 2,
  ) {
    if (version === 2) this.refresh(true); // boot: default face, prog 0, disarmed, full refresh
  }

  get hid(): HidTransport {
    return this.usb !== 'none' ? 'usb' : 'none';
  }

  hello(): string {
    this.bn = newNonceHex();
    this.sk = null;
    this.ek = null;
    this.lastSeq = 0;
    this.n = 0;
    this.lastSentFields = '';
    return `hello usba/${this.version} ${this.boardId} ${this.firmware} power-on ${this.bn}`;
  }

  /** The current state line (USBA/2: signed, `n` rises by one). Null before the handshake on USBA/2. */
  stateLine(): string | null {
    if (this.version === 1) return `s armed=${this.armed ? 1 : 0} usb=${this.usb}`;
    if (!this.sk) return null;
    const fields = this.fields();
    this.lastSentFields = fields;
    this.n += 1;
    const body = `${fields} n=${this.n}`;
    return `s ${body} ${stateTag(this.sk, body)}`;
  }

  /** The arm button: toggles armed. Returns the state line the firmware sends (null with no session). */
  pressButton(): string | null {
    this.setArmed(!this.armed);
    return this.stateLine();
  }

  /**
   * A press the firmware's interrupt latched while its loop was blocked (a
   * relay connect, an mDNS query), or between two lines. The latch only
   * disarms: it is acted on before any later line, so a session formed after
   * it sees a disarmed board. It never arms; arming stays with the held,
   * debounced press (`pressButton`). Returns the state line (null with no session).
   */
  latchPress(): string | null {
    if (this.armed) this.setArmed(false);
    return this.stateLine();
  }

  /** The stop input (dock `STOP_N`): disarms, never arms. */
  pressStop(): string | null {
    if (this.armed) this.setArmed(false);
    return this.stateLine();
  }

  /** Returns the reply line(s), or 'drop' to close the connection. */
  receive(line: string): string[] | 'drop' {
    const trimmed = line.trim();
    const v = this.version;
    if (!this.sk) {
      const m = /^challenge ([0-9a-f]{32}) ([0-9a-f]{64})$/.exec(trimmed);
      if (!m) return 'drop';
      const [, sn, proof] = m;
      if (!equalHex(proof, serverProof(this.keyHex, this.boardId, this.bn, sn, v))) return 'drop';
      this.sk = sessionKey(this.keyHex, this.bn, sn, v);
      this.ek = v === 2 ? textKey(this.keyHex, this.bn, sn) : null;
      return [`auth ${boardProof(this.keyHex, this.boardId, this.bn, sn, v)}`];
    }
    if (trimmed === 'ready') return [this.stateLine()!];
    const m = /^c (\d{1,10}) (.+) ([0-9a-f]{32})$/.exec(trimmed);
    if (!m) return 'drop';
    const seqText = m[1];
    const seq = Number(seqText);
    const command = m[2];
    if (!equalHex(m[3], commandTag(this.sk, seq, command))) return 'drop';
    if (seq <= this.lastSeq) return [this.reply(seqText, 'err replay')];
    this.lastSeq = seq;
    const result = this.run(seq, command);
    if (v === 1) {
      // USBA/1 sends its state after `disarm` and stays silent otherwise.
      return command === 'disarm' ? [this.reply(seqText, result), this.stateLine()!] : [this.reply(seqText, result)];
    }
    const out = [this.reply(seqText, result)];
    if (this.fields() !== this.lastSentFields) out.push(this.stateLine()!);
    return out;
  }

  private run(seq: number, command: string): string {
    const sp = command.indexOf(' ');
    const op = sp < 0 ? command : command.slice(0, sp);
    const arg = sp < 0 ? '' : command.slice(sp + 1);
    if (op === 'ping') return 'ok';
    if (op === 'disarm') {
      this.setArmed(false);
      return 'ok';
    }
    if (this.version === 2) {
      // Display commands sit above the armed check: they never arm, send no
      // HID report, and leave the idle timer alone.
      if (op === 'face') {
        if (!(FACE_NAMES as readonly string[]).includes(arg)) return 'err unknown face';
        this.face = arg as FaceName;
        this.refresh(false);
        return 'ok';
      }
      if (op === 'prog') {
        if (!/^-?\d{1,6}$/.test(arg)) return 'err bad numbers';
        const n = Number(arg);
        if (n < 0 || n > 5 || arg.length !== 1) return 'err out of bounds';
        this.prog = n;
        this.refresh(false);
        return 'ok';
      }
      if (op === 'label') {
        if (arg === '-') {
          this.label = '';
          return 'ok';
        }
        const text = /^(?:[0-9a-f]{2}){1,24}$/.test(arg) ? Buffer.from(arg, 'hex').toString('latin1') : '';
        if (!text || !/^[\x20-\x7E]+$/.test(text)) return 'err bad text';
        this.label = text;
        return 'ok';
      }
    }
    if (!this.armed) return 'err disarmed';
    if (this.usb !== 'mounted') return 'err phone usb asleep';
    if (this.version === 2 && op === 'typex') {
      const text = openTypex(this.ek!, seq, arg);
      if (text === null) return 'err bad text';
      this.log.push(`type ${Buffer.from(text, 'ascii').toString('hex')}`);
      return 'ok';
    }
    if (!INPUT_OPS.has(op)) return 'err unknown command';
    this.log.push(command);
    return 'ok';
  }

  private reply(seqText: string, result: string): string {
    if (this.version === 1) return `r ${seqText} ${result}`;
    return `r ${seqText} ${result} ${hmacHex(this.sk!, `r|${seqText}|${result}`).slice(0, 32)}`;
  }

  private fields(): string {
    return stateFields({ armed: this.armed, usb: this.usb, dock: this.dock, hid: this.hid, link: this.link, face: this.face, prog: this.prog });
  }

  /** The firmware's setArmed(): arming shows `default`; any disarm shows `stopped` with prog 0. */
  private setArmed(on: boolean): void {
    this.armed = on;
    if (this.version === 1) return;
    if (on) this.face = 'default';
    else {
      this.face = 'stopped';
      this.prog = 0;
    }
    this.refresh(false);
  }

  /** Shows a new frame when face, prog or the armed mark changed. Full at boot, for `stopped`, and every 20th time. */
  private refresh(full: boolean): void {
    const key = `${this.face}|${this.prog}|${this.armed ? 1 : 0}`;
    if (!full && key === this.shown) return;
    this.shown = key;
    const f = full || this.face === 'stopped' || this.partials >= FULL_REFRESH_EVERY;
    if (f) this.partials = 0;
    else this.partials += 1;
    this.display.show(renderFace(this.face, this.prog, this.armed), f);
  }
}
